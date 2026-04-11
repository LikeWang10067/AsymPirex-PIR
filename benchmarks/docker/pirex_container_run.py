from __future__ import annotations

import argparse
import json
import math
import os
import re
import socket
import subprocess
import threading
import time
from pathlib import Path


ROOT = Path("/workspace")
PIREX_ROOT = ROOT / "benchmarks" / "external" / "pirex"
LIBS_RS = PIREX_ROOT / "src" / "libs.rs"


class TcpRelay:
    def __init__(self, listen_host: str, listen_port: int, upstream_host: str, upstream_port: int, bandwidth_mbps: float, rtt_ms: float, chunk_bytes: int) -> None:
        self.listen_host = listen_host
        self.listen_port = listen_port
        self.upstream_host = upstream_host
        self.upstream_port = upstream_port
        self.bandwidth_mbps = bandwidth_mbps
        self.rtt_ms = rtt_ms
        self.chunk_bytes = chunk_bytes
        self._server_socket: socket.socket | None = None
        self._thread: threading.Thread | None = None
        self._stop = threading.Event()

    def start(self) -> None:
        self._server_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server_socket.bind((self.listen_host, self.listen_port))
        self._server_socket.listen()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._server_socket is not None:
            try:
                self._server_socket.close()
            except OSError:
                pass
        if self._thread is not None:
            self._thread.join(timeout=2)

    def _run(self) -> None:
        assert self._server_socket is not None
        while not self._stop.is_set():
            try:
                client_socket, _ = self._server_socket.accept()
            except OSError:
                break
            upstream_socket = socket.create_connection((self.upstream_host, self.upstream_port))
            threading.Thread(target=self._forward, args=(client_socket, upstream_socket), daemon=True).start()
            threading.Thread(target=self._forward, args=(upstream_socket, client_socket), daemon=True).start()

    def _forward(self, source: socket.socket, destination: socket.socket) -> None:
        one_way_latency = max(self.rtt_ms / 2.0, 0.0) / 1000.0
        bytes_per_second = (self.bandwidth_mbps * 1_000_000.0) / 8.0
        try:
            while not self._stop.is_set():
                chunk = source.recv(self.chunk_bytes)
                if not chunk:
                    break
                if one_way_latency:
                    time.sleep(one_way_latency)
                if bytes_per_second > 0:
                    time.sleep(len(chunk) / bytes_per_second)
                destination.sendall(chunk)
        except OSError:
            pass
        finally:
            for sock in (source, destination):
                try:
                    sock.close()
                except OSError:
                    pass


def run_command(command: list[str], cwd: Path, env: dict[str, str] | None = None, timeout: float | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout, check=False)


def spawn_command(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> subprocess.Popen[str]:
    return subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        preexec_fn=os.setsid,
    )


def terminate_process_tree(process: subprocess.Popen[str] | None) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, 15)
        process.wait(timeout=5)
    except Exception:
        try:
            os.killpg(process.pid, 9)
        except Exception:
            pass


def parse_duration_ms(content: str, label: str) -> float:
    match = re.findall(rf"{re.escape(label)} ([0-9.]+)(ns|µs|ms|s)", content)
    if not match:
        raise RuntimeError(f"failed to parse Pirex metric: {label}")
    value, unit = match[-1]
    number = float(value)
    if unit == "ns":
        return number / 1_000_000.0
    if unit == "µs":
        return number / 1000.0
    if unit == "ms":
        return number
    return number * 1000.0


def parse_int(content: str, label: str) -> int:
    match = re.findall(rf"{re.escape(label)} ([0-9]+)", content)
    if not match:
        raise RuntimeError(f"failed to parse Pirex metric: {label}")
    return int(match[-1])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--logical-db-size", type=int, required=True)
    parser.add_argument("--record-size", type=int, required=True)
    parser.add_argument("--query-count", type=int, required=True)
    parser.add_argument("--bandwidth-mbps", type=float, required=True)
    parser.add_argument("--rtt-ms", type=float, required=True)
    parser.add_argument("--chunk-bytes", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if args.record_size % 64 != 0:
        raise SystemExit("Pirex requires record sizes that are multiples of 64 bytes.")
    if args.logical_db_size & (args.logical_db_size - 1):
        raise SystemExit("Pirex config.py expects a power-of-two database size.")

    exponent = int(math.log2(args.logical_db_size))
    chunk_count = args.record_size // 64
    relay = None
    server_process = None
    prep_server_process = None
    relay_host = "127.0.0.1"
    relay_port = 8112
    server_port = 8111
    build_env = os.environ.copy()
    build_env["RUSTUP_TOOLCHAIN"] = "nightly-2023-09-24"
    original_libs = LIBS_RS.read_text()

    for relative in [
        "client_prep_auto",
        "server_prep_auto",
        "results/pirex_client_online.txt",
        "results/pirex_server_online.txt",
    ]:
        (PIREX_ROOT / relative).unlink(missing_ok=True)

    try:
        LIBS_RS.write_text(
            original_libs.replace(
                'pub const SERVER_ADDRESS: &str = "127.0.0.1:8111";',
                f'pub const SERVER_ADDRESS: &str = "{relay_host}:{relay_port}";',
                1,
            )
        )

        completed = run_command(["python3", "config.py", str(chunk_count), str(exponent)], cwd=PIREX_ROOT, timeout=120)
        if completed.returncode != 0:
            raise RuntimeError(completed.stderr or completed.stdout)

        build = run_command(
            [
                "cargo",
                "+nightly-2023-09-24",
                "build",
                "--release",
                "--bin",
                "helper",
                "--bin",
                "pirex_sprep",
                "--bin",
                "pirex_uprep",
                "--bin",
                "pirex_sread",
                "--bin",
                "pirex_uread",
            ],
            cwd=PIREX_ROOT,
            env=build_env,
            timeout=3600,
        )
        if build.returncode != 0:
            raise RuntimeError(build.stderr or build.stdout)

        helper = run_command(["./target/release/helper"], cwd=PIREX_ROOT, env=build_env, timeout=1800)
        if helper.returncode != 0:
            raise RuntimeError(helper.stderr or helper.stdout)

        relay = TcpRelay(relay_host, relay_port, relay_host, server_port, args.bandwidth_mbps, args.rtt_ms, args.chunk_bytes)
        relay.start()

        prep_server_process = spawn_command(["./target/release/pirex_sprep"], cwd=PIREX_ROOT, env=build_env)
        time.sleep(2)
        prep_client = run_command(["./target/release/pirex_uprep"], cwd=PIREX_ROOT, env=build_env, timeout=1800)
        if prep_client.returncode != 0:
            raise RuntimeError(prep_client.stderr or prep_client.stdout)
        time.sleep(2)
        terminate_process_tree(prep_server_process)
        prep_server_process = None

        server_process = spawn_command(["./target/release/pirex_sread"], cwd=PIREX_ROOT, env=build_env)
        time.sleep(2)
        client = run_command(["./target/release/pirex_uread"], cwd=PIREX_ROOT, env=build_env, timeout=1800)
        if client.returncode != 0:
            raise RuntimeError(client.stderr or client.stdout)
        time.sleep(2)
        terminate_process_tree(server_process)
        server_process = None

        offline_client_ms = parse_duration_ms((PIREX_ROOT / "client_prep_auto").read_text(), "client prep elapse")
        offline_server_ms = parse_duration_ms((PIREX_ROOT / "server_prep_auto").read_text(), "server XX prep elapse")
        client_online = (PIREX_ROOT / "results/pirex_client_online.txt").read_text()
        server_online = (PIREX_ROOT / "results/pirex_server_online.txt").read_text()
        client_compute_ms = parse_duration_ms(client_online, "client computation elapse")
        online_band_ms = parse_duration_ms(client_online, "client request elapse")
        outbound_bytes = parse_int(client_online, "total bandwidth nbytes")
        server_compute_ms = parse_duration_ms(server_online, "server computation elapse")
        offline_ms = max(offline_client_ms, offline_server_ms)
        online_total_ms = max(client_compute_ms, server_compute_ms) + online_band_ms
        amortized_ms = (offline_ms + online_total_ms * args.query_count) / args.query_count

        payload = {
            "scheme": "Pirex",
            "trust_model": "two-server offline-online",
            "topology": "tcp client-server in Linux container",
            "logical_db_size": args.logical_db_size,
            "record_size_bytes": args.record_size,
            "query_count": args.query_count,
            "bandwidth_mbps": args.bandwidth_mbps,
            "rtt_ms": args.rtt_ms,
            "offline_ms": offline_ms,
            "online_total_ms": online_total_ms,
            "online_avg_ms": online_total_ms / args.query_count,
            "amortized_ms": amortized_ms,
            "client_outbound_bytes": outbound_bytes,
            "client_inbound_bytes": outbound_bytes,
            "client_storage_bytes": None,
            "client_compute_ms": client_compute_ms,
            "server_compute_ms": server_compute_ms,
            "notes": [
                "Pirex is executed inside a Linux container on macOS because the vendored native artifacts do not link cleanly on Darwin.",
                "A TCP relay runs in-container so the same bandwidth and RTT profile is applied during the Pirex phases.",
            ],
            "extra": {
                "offline_client_ms": offline_client_ms,
                "offline_server_ms": offline_server_ms,
                "client_request_ms": online_band_ms,
            },
        }

        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(payload, indent=2))
    finally:
        LIBS_RS.write_text(original_libs)
        if relay is not None:
            relay.stop()
        terminate_process_tree(server_process)
        terminate_process_tree(prep_server_process)


if __name__ == "__main__":
    main()
