from __future__ import annotations

import csv
import json
import os
import shutil
import signal
import socket
import subprocess
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

import psutil


ROOT = Path(__file__).resolve().parents[2]
RAW_RESULTS_DIR = ROOT / "benchmarks" / "results" / "raw"
SUMMARY_RESULTS_DIR = ROOT / "benchmarks" / "results" / "summary"


@dataclass
class NetworkProfile:
    bandwidth_mbps: float
    rtt_ms: float
    chunk_bytes: int = 4096


@dataclass
class BenchmarkCase:
    case_id: str
    logical_db_size: int
    record_size_bytes: int
    query_count: int
    active_hint_count: int
    refresh_batch_size: int
    schemes: list[str]


@dataclass
class UnifiedResult:
    scheme: str
    case_id: str
    success: bool
    trust_model: str
    topology: str
    logical_db_size: int
    record_size_bytes: int
    query_count: int
    repetitions: int
    bandwidth_mbps: float
    rtt_ms: float
    offline_ms: float | None = None
    query_only_total_ms: float | None = None
    query_only_avg_ms: float | None = None
    online_total_ms: float | None = None
    online_avg_ms: float | None = None
    amortized_ms: float | None = None
    maintenance_total_ms: float | None = None
    refresh_count: int | None = None
    client_outbound_bytes: int | None = None
    client_inbound_bytes: int | None = None
    maintenance_outbound_bytes: int | None = None
    maintenance_inbound_bytes: int | None = None
    client_storage_bytes: int | None = None
    client_compute_ms: float | None = None
    server_compute_ms: float | None = None
    notes: list[str] = field(default_factory=list)
    extra: dict[str, Any] = field(default_factory=dict)
    error: str | None = None


def load_config(path: Path) -> tuple[dict[str, Any], list[BenchmarkCase], NetworkProfile]:
    config = json.loads(path.read_text())
    network = NetworkProfile(**config["network"])
    cases = [BenchmarkCase(**entry) for entry in config["matrix"]]
    return config, cases, network


def ensure_results_dirs() -> None:
    RAW_RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    SUMMARY_RESULTS_DIR.mkdir(parents=True, exist_ok=True)


def write_raw_result(result: UnifiedResult) -> Path:
    ensure_results_dirs()
    output_path = RAW_RESULTS_DIR / f"{result.case_id}_{result.scheme}.json"
    output_path.write_text(json.dumps(asdict(result), indent=2))
    return output_path


def write_summary(results: list[UnifiedResult]) -> tuple[Path, Path]:
    ensure_results_dirs()
    raw_path = SUMMARY_RESULTS_DIR / "raw.json"
    csv_path = SUMMARY_RESULTS_DIR / "summary.csv"

    raw_path.write_text(json.dumps([asdict(item) for item in results], indent=2))

    fieldnames = [
        "scheme",
        "case_id",
        "success",
        "trust_model",
        "topology",
        "logical_db_size",
        "record_size_bytes",
        "query_count",
        "repetitions",
        "bandwidth_mbps",
        "rtt_ms",
        "offline_ms",
        "query_only_total_ms",
        "query_only_avg_ms",
        "online_total_ms",
        "online_avg_ms",
        "amortized_ms",
        "maintenance_total_ms",
        "refresh_count",
        "client_outbound_bytes",
        "client_inbound_bytes",
        "maintenance_outbound_bytes",
        "maintenance_inbound_bytes",
        "client_storage_bytes",
        "client_compute_ms",
        "server_compute_ms",
        "error",
        "notes",
    ]

    with csv_path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for item in results:
            row = asdict(item)
            row["notes"] = " | ".join(item.notes)
            writer.writerow({key: row.get(key) for key in fieldnames})

    return raw_path, csv_path


def run_command(
    command: list[str],
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )


def spawn_command(
    command: list[str],
    cwd: Path,
    env: dict[str, str] | None = None,
) -> subprocess.Popen[str]:
    return subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        preexec_fn=os.setsid if os.name != "nt" else None,
    )


def terminate_process_tree(process: subprocess.Popen[Any] | None) -> None:
    if process is None or process.poll() is not None:
        return

    try:
        parent = psutil.Process(process.pid)
    except psutil.Error:
        return

    children = parent.children(recursive=True)
    for child in children:
        try:
            child.terminate()
        except psutil.Error:
            pass
    parent.terminate()

    gone, alive = psutil.wait_procs(children + [parent], timeout=5)
    for proc in alive:
        try:
            proc.kill()
        except psutil.Error:
            pass


def require_executable(name: str) -> None:
    if shutil.which(name) is None:
        raise RuntimeError(f"required executable not found: {name}")


def parse_first_float(text: str) -> float:
    token = text.strip().split()[-1]
    return float(token)


def parse_duration_ms(text: str) -> float:
    token = text.strip().split()[-1]
    if token.endswith("ms"):
        return float(token[:-2])
    if token.endswith("ns"):
        return float(token[:-2]) / 1_000_000.0
    if token.endswith("µs"):
        return float(token[:-2]) / 1000.0
    if token.endswith("s"):
        return float(token[:-1]) * 1000.0
    return float(token)


class TcpRelay:
    def __init__(self, listen_host: str, listen_port: int, upstream_host: str, upstream_port: int, network: NetworkProfile):
        self.listen_host = listen_host
        self.listen_port = listen_port
        self.upstream_host = upstream_host
        self.upstream_port = upstream_port
        self.network = network
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
            threading.Thread(
                target=self._forward,
                args=(client_socket, upstream_socket),
                daemon=True,
            ).start()
            threading.Thread(
                target=self._forward,
                args=(upstream_socket, client_socket),
                daemon=True,
            ).start()

    def _forward(self, source: socket.socket, destination: socket.socket) -> None:
        one_way_latency = max(self.network.rtt_ms / 2.0, 0.0) / 1000.0
        bytes_per_second = (self.network.bandwidth_mbps * 1_000_000.0) / 8.0

        try:
            while not self._stop.is_set():
                chunk = source.recv(self.network.chunk_bytes)
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
