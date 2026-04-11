from __future__ import annotations

import json
from pathlib import Path

from .common import (
    BenchmarkCase,
    NetworkProfile,
    ROOT,
    UnifiedResult,
    run_command,
)


def run_pirex(case: BenchmarkCase, network: NetworkProfile, repetitions: int) -> UnifiedResult:
    if case.record_size_bytes % 64 != 0:
        return _failed_result(
            case,
            network,
            repetitions,
            "Pirex requires record sizes that are multiples of 64 bytes.",
        )
    if case.logical_db_size & (case.logical_db_size - 1):
        return _failed_result(
            case,
            network,
            repetitions,
            "Pirex requires a power-of-two database size.",
        )

    output_path = ROOT / "benchmarks" / "results" / "raw" / f"{case.case_id}_pirex_reimpl_native.json"
    command = [
        "cargo",
        "run",
        "--release",
        "--manifest-path",
        str(ROOT / "asympirex" / "Cargo.toml"),
        "--bin",
        "pirex_reimpl_bench",
        "--",
        "--total-records",
        str(case.logical_db_size),
        "--record-size",
        str(case.record_size_bytes),
        "--query-count",
        str(case.query_count),
        "--bandwidth-mbps",
        str(network.bandwidth_mbps),
        "--rtt-ms",
        str(network.rtt_ms),
        "--output",
        str(output_path),
    ]

    completed = run_command(command, cwd=ROOT, timeout=1200)
    if completed.returncode != 0:
        return _failed_result(case, network, repetitions, completed.stderr or completed.stdout)

    payload = json.loads(output_path.read_text())
    return UnifiedResult(
        scheme="Pirex",
        case_id=case.case_id,
        success=True,
        trust_model=payload["trust_model"],
        topology=payload["topology"],
        logical_db_size=payload["total_records"],
        record_size_bytes=payload["record_size_bytes"],
        query_count=payload["query_count"],
        repetitions=repetitions,
        bandwidth_mbps=payload["network_bandwidth_mbps"],
        rtt_ms=payload["network_rtt_ms"],
        offline_ms=payload["offline_ms"],
        query_only_total_ms=payload["online_end_to_end_total_ms"],
        query_only_avg_ms=payload["online_end_to_end_avg_ms"],
        online_total_ms=payload["online_end_to_end_total_ms"],
        online_avg_ms=payload["online_end_to_end_avg_ms"],
        amortized_ms=payload["amortized_ms"],
        client_outbound_bytes=payload["offline_client_outbound_bytes"] + payload["online_client_outbound_bytes"],
        client_inbound_bytes=payload["offline_client_inbound_bytes"] + payload["online_client_inbound_bytes"],
        client_storage_bytes=payload["client_storage_bytes"],
        client_compute_ms=payload["online_client_compute_total_ms"],
        server_compute_ms=payload["online_server_compute_total_ms"],
        notes=[
            "Pirex is a local Rust reconstruction of the base artifact's prep/query flow.",
            "Network cost is analytically derived from the reconstructed message sizes under the shared profile.",
        ],
        extra=payload,
    )


def _failed_result(
    case: BenchmarkCase,
    network: NetworkProfile,
    repetitions: int,
    error: str,
) -> UnifiedResult:
    return UnifiedResult(
        scheme="Pirex",
        case_id=case.case_id,
        success=False,
        trust_model="two-server offline-online",
        topology="tcp client-server",
        logical_db_size=case.logical_db_size,
        record_size_bytes=case.record_size_bytes,
        query_count=case.query_count,
        repetitions=repetitions,
        bandwidth_mbps=network.bandwidth_mbps,
        rtt_ms=network.rtt_ms,
        error=error,
    )
