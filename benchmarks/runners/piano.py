from __future__ import annotations

import re
import time
from contextlib import suppress
from pathlib import Path

from .common import (
    BenchmarkCase,
    NetworkProfile,
    ROOT,
    TcpRelay,
    UnifiedResult,
    require_executable,
    run_command,
    spawn_command,
    terminate_process_tree,
)


PIANO_ROOT = ROOT / "benchmarks" / "external" / "Piano-PIR-new"
CONFIG_PATH = PIANO_ROOT / "config.txt"
UTIL_PATH = PIANO_ROOT / "util" / "util.go"
OUTPUT_PATH = PIANO_ROOT / "output.txt"


def run_piano(case: BenchmarkCase, network: NetworkProfile, repetitions: int) -> UnifiedResult:
    try:
        require_executable("go")
    except RuntimeError as exc:
        return UnifiedResult(
            scheme="Piano",
            case_id=case.case_id,
            success=False,
            trust_model="single-server",
            topology="client-server over TCP relay",
            logical_db_size=case.logical_db_size,
            record_size_bytes=case.record_size_bytes,
            query_count=case.query_count,
            repetitions=repetitions,
            bandwidth_mbps=network.bandwidth_mbps,
            rtt_ms=network.rtt_ms,
            error=str(exc),
        )

    original_config = CONFIG_PATH.read_text()
    original_util = UTIL_PATH.read_text()
    server_process = None
    relay = None

    try:
        CONFIG_PATH.write_text(f"{case.logical_db_size} 1\n")
        UTIL_PATH.write_text(_rewrite_db_entry_size(original_util, case.record_size_bytes))
        OUTPUT_PATH.unlink(missing_ok=True)

        server_port = 50052
        relay_port = 50051
        relay = TcpRelay("127.0.0.1", relay_port, "127.0.0.1", server_port, network)
        relay.start()

        server_process = spawn_command(
            ["go", "run", "server/server.go", "-port", str(server_port)],
            cwd=PIANO_ROOT,
        )
        time.sleep(3)

        client_result = run_command(
            [
                "go",
                "run",
                "client_new/client_new.go",
                "-ip",
                f"127.0.0.1:{relay_port}",
                "-thread",
                "1",
            ],
            cwd=PIANO_ROOT,
            timeout=1800,
        )
        if client_result.returncode != 0:
            raise RuntimeError(client_result.stderr or client_result.stdout)

        metrics = _parse_output_file(OUTPUT_PATH)
        query_count = int(metrics["query_count"])
        per_query_upload_kb = metrics["per_query_upload_kb"]
        per_query_download_kb = metrics["per_query_download_kb"]

        return UnifiedResult(
            scheme="Piano",
            case_id=case.case_id,
            success=True,
            trust_model="single-server",
            topology="client-server over TCP relay",
            logical_db_size=case.logical_db_size,
            record_size_bytes=case.record_size_bytes,
            query_count=query_count,
            repetitions=repetitions,
            bandwidth_mbps=network.bandwidth_mbps,
            rtt_ms=network.rtt_ms,
            offline_ms=metrics["offline_ms"],
            query_only_total_ms=metrics["online_ms"],
            query_only_avg_ms=metrics["online_ms"] / query_count if query_count else None,
            online_total_ms=metrics["online_ms"],
            online_avg_ms=metrics["online_ms"] / query_count if query_count else None,
            amortized_ms=metrics["amortized_ms"],
            client_outbound_bytes=int(per_query_upload_kb * 1024 * query_count),
            client_inbound_bytes=int(per_query_download_kb * 1024 * query_count),
            client_storage_bytes=int(metrics["client_storage_mb"] * 1024 * 1024),
            client_compute_ms=metrics["average_client_time_ms"],
            server_compute_ms=metrics["average_server_time_ms"],
            notes=[
                "Offline and online numbers are parsed from Piano's native output.txt.",
                "Network shaping is applied through a TCP relay between client and server.",
            ],
            extra=metrics,
        )
    except Exception as exc:  # noqa: BLE001
        return UnifiedResult(
            scheme="Piano",
            case_id=case.case_id,
            success=False,
            trust_model="single-server",
            topology="client-server over TCP relay",
            logical_db_size=case.logical_db_size,
            record_size_bytes=case.record_size_bytes,
            query_count=case.query_count,
            repetitions=repetitions,
            bandwidth_mbps=network.bandwidth_mbps,
            rtt_ms=network.rtt_ms,
            error=str(exc),
        )
    finally:
        if relay is not None:
            relay.stop()
        terminate_process_tree(server_process)
        CONFIG_PATH.write_text(original_config)
        UTIL_PATH.write_text(original_util)


def _rewrite_db_entry_size(contents: str, record_size_bytes: int) -> str:
    return re.sub(
        r"DBEntrySize\s*=\s*\d+",
        f"DBEntrySize   = {record_size_bytes}",
        contents,
        count=1,
    )


def _parse_output_file(path: Path) -> dict[str, float]:
    content = path.read_text()
    patterns = {
        "client_storage_mb": r"Local Storage Size ([0-9.]+) MB",
        "offline_ms": r"Setup Phase took ([0-9.]+) ms",
        "online_ms": r"Online Phase took ([0-9.]+) ms",
        "per_query_upload_kb": r"Per query upload cost ([0-9.]+) kb",
        "per_query_download_kb": r"Per query download cost ([0-9.]+) kb",
        "amortized_ms": r"End to end amortized time ([0-9.]+) ms",
        "average_server_time_ms": r"Average Server Time ([0-9.]+) ms",
        "average_client_time_ms": r"Average Client Time ([0-9.]+) ms",
        "query_count": r"Finish Online Phase with ([0-9]+) queries",
    }

    parsed: dict[str, float] = {}
    for key, pattern in patterns.items():
        matches = re.findall(pattern, content)
        if not matches:
            raise RuntimeError(f"failed to parse Piano metric: {key}")
        parsed[key] = float(matches[-1])
    return parsed
