// src/main.rs
use asympirex::client::{self, Client, ClientStateDelta, ClientStateSnapshot};
use asympirex::database::Database;
use serde::Serialize;
use asympirex::server::{HintServer, QueryServer};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize)]
struct QueryTraceEntry {
    query_index: usize,
    outcome: client::QueryOutcome,
    before: ClientStateSnapshot,
    after: ClientStateSnapshot,
    delta: ClientStateDelta,
}

#[derive(Serialize)]
struct RunTrace {
    parameters: RunParameters,
    initial_snapshot: ClientStateSnapshot,
    queries: Vec<QueryTraceEntry>,
    final_snapshot: ClientStateSnapshot,
}

#[derive(Serialize)]
struct RunParameters {
    n: usize,
    m: usize,
    record_size: usize,
    active_hint_count: usize,
    refresh_batch_size: usize,
    targets: Vec<usize>,
}

fn main() {
    let mut run_log = String::new();
    writeln!(&mut run_log, "=== AsymPirex end-to-end prototype ===").unwrap();

    let n = 4;
    let m = 4;
    let b = 16;
    let active_hint_count = 64;
    let refresh_batch_size = 8;
    let master_secret = [0x5Au8; 32];

    let mut db = Database::new(n, m, b);

    for index in 0..db.total_records {
        let value = (index as u8).wrapping_mul(17).wrapping_add(3);
        db.set_record(index, vec![value; b]);
    }

    let hint_server = HintServer::new(&db, master_secret);
    let query_server = QueryServer::new(&db);

    writeln!(
        &mut run_log,
        "Initialized DB with N = {}, n = {}, m = {}, record size = {} bytes.",
        db.total_records, n, m, b
    )
    .unwrap();

    let mut client = Client::bootstrap_from_hint_server(
        "client-001",
        n,
        m,
        active_hint_count,
        refresh_batch_size,
        &hint_server,
    );

    writeln!(
        &mut run_log,
        "Client preprocessing complete: {} active hints, {} buffered hints.",
        client.active_hint_len(),
        client.buffer_len()
    )
    .unwrap();
    let initial_snapshot = client.snapshot();
    append_client_snapshot_preview(
        &mut run_log,
        "After Initialization: client's 3 local tables",
        &initial_snapshot,
    );

    let targets = [5usize, 6, 10, 15, 5, 1, 14, 3, 12];
    let mut query_traces = Vec::new();
    let last_query_index = targets.len() - 1;
    for (query_index, target) in targets.into_iter().enumerate() {
        let before = client.snapshot();
        let outcome = client
            .query(target, &query_server, &hint_server)
            .unwrap_or_else(|err| panic!("query for target {target} failed: {err}"));
        let after = client.snapshot();
        let delta = Client::snapshot_delta(&before, &after);

        let expected = db
            .get_record(target)
            .cloned()
            .expect("target record must exist in the database");

        assert_eq!(
            outcome.target_record, expected,
            "recovered record does not match DB[{target}]"
        );

        if query_index == 0 || query_index == last_query_index {
            append_query_report(
                &mut run_log,
                if query_index == 0 {
                    "First query"
                } else {
                    "Last query"
                },
                &outcome,
                &after,
            );
        }

        query_traces.push(QueryTraceEntry {
            query_index,
            outcome,
            before,
            after,
            delta,
        });
    }

    writeln!(
        &mut run_log,
        "Final client state: {} active hints, {} buffered hints.",
        client.active_hint_len(),
        client.buffer_len()
    )
    .unwrap();
    let final_snapshot = client.snapshot();
    let trace = RunTrace {
        parameters: RunParameters {
            n,
            m,
            record_size: b,
            active_hint_count,
            refresh_batch_size,
            targets: targets.to_vec(),
        },
        initial_snapshot,
        queries: query_traces,
        final_snapshot,
    };
    let (trace_path, log_path) = write_output_files(&trace, &run_log);
    writeln!(&mut run_log, "Wrote JSON trace to {}", trace_path.display()).unwrap();
    writeln!(&mut run_log, "Wrote text log to {}", log_path.display()).unwrap();
    writeln!(&mut run_log, "AsymPirex prototype completed successfully.").unwrap();
    fs::write(&log_path, run_log).expect("failed to write text log");
}

fn append_query_report(
    buffer: &mut String,
    label: &str,
    outcome: &client::QueryOutcome,
    after: &ClientStateSnapshot,
) {
    writeln!(buffer, "\n=== {label} ===").unwrap();
    writeln!(buffer, "Client wants: DB[{}]", outcome.target_index).unwrap();
    writeln!(
        buffer,
        "Client sent: patched offsets={:?}, patched masks={}",
        outcome.sent_query.offsets,
        format_masks(&outcome.sent_query.masks)
    )
    .unwrap();
    writeln!(
        buffer,
        "Data recovery: consumed hint #{}, swap hint {:?}, z = {}, recovered target = {:?}, learned z-record = {:?}, refresh = {}",
        outcome.consumed_hint_id,
        outcome.swap_hint_id,
        outcome.replacement_index,
        outcome.target_record,
        outcome.replacement_record,
        outcome.refresh_triggered
    )
    .unwrap();
    append_client_snapshot_preview(buffer, "Client's 3 local tables after maintenance", after);
}

fn append_client_snapshot_preview(buffer: &mut String, title: &str, snapshot: &ClientStateSnapshot) {
    writeln!(buffer, "\n=== {title} ===").unwrap();
    append_preview_section(
        buffer,
        "hint (active hints)",
        &snapshot
            .active_hints
            .iter()
            .map(|hint| {
                format!(
                    "hint #{}: indices={:?}, offsets={:?}, masks={}, rho_s={:?}, rho_v={:?}",
                    hint.hint_id,
                    hint.indices,
                    hint.offsets,
                    format_masks(&hint.masks),
                    hint.rho_s,
                    hint.rho_v
                )
            })
            .collect::<Vec<_>>(),
    );
    append_preview_section(
        buffer,
        "hbuffer (queued compact tuples)",
        &snapshot
            .hint_buffer
            .iter()
            .map(|hint| format!("hint #{}: rho_s={:?}, rho_v={:?}", hint.hint_id, hint.rho_s, hint.rho_v))
            .collect::<Vec<_>>(),
    );
    append_preview_section(
        buffer,
        "T (hint-membership table)",
        &snapshot
            .membership_table
            .iter()
            .map(|entry| format!("DB[{}] -> {:?}", entry.db_index, entry.hint_ids))
            .collect::<Vec<_>>(),
    );
}

fn append_preview_section(buffer: &mut String, title: &str, entries: &[String]) {
    writeln!(buffer, "{title}:").unwrap();
    let total = entries.len();

    if total == 0 {
        writeln!(buffer, "  <empty>").unwrap();
        return;
    }

    if total <= 6 {
        for entry in entries {
            writeln!(buffer, "  {entry}").unwrap();
        }
        return;
    }

    for entry in entries {
        writeln!(buffer, "  {entry}").unwrap();
    }
}

fn format_masks(masks: &[bool]) -> String {
    let digits = masks
        .iter()
        .map(|bit| if *bit { "1" } else { "0" })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{digits}]")
}

fn write_output_files(trace: &RunTrace, run_log: &str) -> (PathBuf, PathBuf) {
    let output_dir = PathBuf::from("asympirex/output");
    fs::create_dir_all(&output_dir).expect("failed to create output directory");
    let trace_path = output_dir.join("client_trace.json");
    let log_path = output_dir.join("client_run.log");
    let json = serde_json::to_string_pretty(trace).expect("failed to serialize JSON trace");
    fs::write(&trace_path, json).expect("failed to write JSON trace");
    fs::write(&log_path, run_log).expect("failed to write text log");
    (trace_path, log_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_database(n: usize, m: usize, record_size: usize) -> Database {
        let mut db = Database::new(n, m, record_size);
        for index in 0..db.total_records {
            let value = (index as u8).wrapping_mul(17).wrapping_add(3);
            db.set_record(index, vec![value; record_size]);
        }
        db
    }

    #[test]
    fn recovers_records_and_triggers_refresh() {
        let n = 4;
        let m = 4;
        let record_size = 16;
        let active_hint_count = 64;
        let refresh_batch_size = 4;
        let master_secret = [0x5Au8; 32];

        let db = build_database(n, m, record_size);
        let hint_server = HintServer::new(&db, master_secret);
        let query_server = QueryServer::new(&db);
        let mut client = Client::bootstrap_from_hint_server(
            "test-client",
            n,
            m,
            active_hint_count,
            refresh_batch_size,
            &hint_server,
        );

        let mut refresh_seen = false;
        for target in [5usize, 6, 10, 15, 1] {
            let outcome = client.query(target, &query_server, &hint_server).unwrap();
            let expected = db.get_record(target).unwrap().clone();
            assert_eq!(outcome.target_record, expected);
            refresh_seen |= outcome.refresh_triggered;
        }

        assert!(refresh_seen, "expected at least one asynchronous refresh");
        assert_eq!(client.active_hint_len(), active_hint_count);
        assert!(client.buffer_len() > 0, "buffer should remain populated");
    }
}