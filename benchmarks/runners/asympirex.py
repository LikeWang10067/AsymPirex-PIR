from __future__ import annotations

import json
from pathlib import Path

from .common import BenchmarkCase, NetworkProfile, ROOT, UnifiedResult, run_command


def run_asympirex(case: BenchmarkCase, network: NetworkProfile, repetitions: int) -> UnifiedResult:
    output_path = ROOT / "benchmarks" / "results" / "raw" / f"{case.case_id}_asympirex_native.json"

    command = [
        "cargo",
        "run",
        "--release",
        "--manifest-path",
        str(ROOT / "asympirex" / "Cargo.toml"),
        "--bin",
        "asympirex_bench",
        "--",
        "--n",
        str(_choose_partition_count(case.logical_db_size)),
        "--m",
        str(_choose_records_per_partition(case.logical_db_size)),
        "--record-size",
        str(case.record_size_bytes),
        "--active-hints",
        str(case.active_hint_count),
        "--refresh-batch",
        str(case.refresh_batch_size),
        "--query-count",
        str(case.query_count),
        "--bandwidth-mbps",
        str(network.bandwidth_mbps),
        "--rtt-ms",
        str(network.rtt_ms),
        "--output",
        str(output_path),
    ]

    completed = run_command(command, cwd=ROOT, timeout=600)
    if completed.returncode != 0:
        return UnifiedResult(
            scheme="AsymPirex",
            case_id=case.case_id,
            success=False,
            trust_model="two-server offline-online",
            topology="in-process analytical network",
            logical_db_size=case.logical_db_size,
            record_size_bytes=case.record_size_bytes,
            query_count=case.query_count,
            repetitions=repetitions,
            bandwidth_mbps=network.bandwidth_mbps,
            rtt_ms=network.rtt_ms,
            error=completed.stderr or completed.stdout,
        )

    payload = json.loads(output_path.read_text())
    return UnifiedResult(
        scheme="AsymPirex",
        case_id=case.case_id,
        success=True,
        trust_model=payload["trust_model"],
        topology="in-process analytical network",
        logical_db_size=payload["total_records"],
        record_size_bytes=payload["record_size_bytes"],
        query_count=payload["query_count"],
        repetitions=repetitions,
        bandwidth_mbps=payload["network_bandwidth_mbps"],
        rtt_ms=payload["network_rtt_ms"],
        offline_ms=payload["offline_wall_ms"],
        query_only_total_ms=payload["online_end_to_end_total_ms"],
        query_only_avg_ms=payload["online_end_to_end_avg_ms"],
        online_total_ms=payload["online_end_to_end_total_ms"],
        online_avg_ms=payload["online_end_to_end_avg_ms"],
        amortized_ms=payload["amortized_end_to_end_ms"],
        maintenance_total_ms=payload.get("maintenance_wall_total_ms"),
        refresh_count=payload.get("refresh_count"),
        client_outbound_bytes=payload["offline_client_outbound_bytes"]
        + payload["online_client_outbound_bytes"]
        + payload.get("maintenance_client_outbound_bytes", 0),
        client_inbound_bytes=payload["offline_client_inbound_bytes"]
        + payload["online_client_inbound_bytes"]
        + payload.get("maintenance_client_inbound_bytes", 0),
        maintenance_outbound_bytes=payload.get("maintenance_client_outbound_bytes"),
        maintenance_inbound_bytes=payload.get("maintenance_client_inbound_bytes"),
        client_storage_bytes=payload["client_storage_bytes"],
        client_compute_ms=payload["online_wall_total_ms"],
        server_compute_ms=None,
        notes=[
            "Query-only online latency is reported separately from maintenance-aware amortized latency.",
            "Network cost is analytically derived from compact message sizes.",
        ],
        extra=payload,
    )


def _choose_partition_count(total_records: int) -> int:
    root = int(total_records ** 0.5)
    while root > 1 and total_records % root != 0:
        root -= 1
    return root


def _choose_records_per_partition(total_records: int) -> int:
    n = _choose_partition_count(total_records)
    return total_records // n
