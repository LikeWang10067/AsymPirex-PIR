use asympirex::client::Client;
use asympirex::database::Database;
use asympirex::server::{HintServer, QueryServer};
use serde::Serialize;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    n: usize,
    m: usize,
    record_size: usize,
    active_hint_count: usize,
    refresh_batch_size: usize,
    query_count: usize,
    bandwidth_mbps: f64,
    rtt_ms: f64,
    client_id: String,
    output_path: PathBuf,
}

#[derive(Serialize)]
struct AsympirexBenchmarkResult {
    scheme: &'static str,
    trust_model: &'static str,
    total_records: usize,
    partition_count: usize,
    records_per_partition: usize,
    record_size_bytes: usize,
    query_count: usize,
    active_hint_count: usize,
    refresh_batch_size: usize,
    network_bandwidth_mbps: f64,
    network_rtt_ms: f64,
    offline_wall_ms: f64,
    offline_client_outbound_bytes: usize,
    offline_client_inbound_bytes: usize,
    maintenance_wall_total_ms: f64,
    maintenance_client_outbound_bytes: usize,
    maintenance_client_inbound_bytes: usize,
    refresh_count: usize,
    refresh_hint_count: usize,
    online_wall_total_ms: f64,
    online_wall_avg_ms: f64,
    online_network_total_ms: f64,
    online_end_to_end_total_ms: f64,
    online_end_to_end_avg_ms: f64,
    online_client_outbound_bytes: usize,
    online_client_inbound_bytes: usize,
    amortized_end_to_end_ms: f64,
    client_storage_bytes: usize,
}

fn main() {
    let config = parse_args();
    let result = run_benchmark(&config);

    if let Some(parent) = config.output_path.parent() {
        fs::create_dir_all(parent).expect("failed to create benchmark output directory");
    }

    let json = serde_json::to_string_pretty(&result).expect("failed to serialize benchmark result");
    fs::write(&config.output_path, json).expect("failed to write benchmark result");
}

fn parse_args() -> BenchmarkConfig {
    let mut n = 32usize;
    let mut m = 32usize;
    let mut record_size = 64usize;
    let mut active_hint_count = 128usize;
    let mut refresh_batch_size = 16usize;
    let mut query_count = 20usize;
    let mut bandwidth_mbps = 40.0f64;
    let mut rtt_ms = 60.0f64;
    let mut client_id = String::from("bench-client");
    let mut output_path = PathBuf::from("asympirex/output/bench_result.json");

    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_else(|| panic!("missing value for {flag}"));
        match flag.as_str() {
            "--n" => n = value.parse().expect("invalid --n"),
            "--m" => m = value.parse().expect("invalid --m"),
            "--record-size" => record_size = value.parse().expect("invalid --record-size"),
            "--active-hints" => {
                active_hint_count = value.parse().expect("invalid --active-hints")
            }
            "--refresh-batch" => {
                refresh_batch_size = value.parse().expect("invalid --refresh-batch")
            }
            "--query-count" => query_count = value.parse().expect("invalid --query-count"),
            "--bandwidth-mbps" => {
                bandwidth_mbps = value.parse().expect("invalid --bandwidth-mbps")
            }
            "--rtt-ms" => rtt_ms = value.parse().expect("invalid --rtt-ms"),
            "--client-id" => client_id = value,
            "--output" => output_path = PathBuf::from(value),
            other => panic!("unknown flag {other}"),
        }
    }

    BenchmarkConfig {
        n,
        m,
        record_size,
        active_hint_count,
        refresh_batch_size,
        query_count,
        bandwidth_mbps,
        rtt_ms,
        client_id,
        output_path,
    }
}

fn run_benchmark(config: &BenchmarkConfig) -> AsympirexBenchmarkResult {
    let mut db = Database::new(config.n, config.m, config.record_size);
    for index in 0..db.total_records {
        let value = (index as u8).wrapping_mul(17).wrapping_add(3);
        db.set_record(index, vec![value; config.record_size]);
    }

    let master_secret = [0x5Au8; 32];
    let hint_server = HintServer::new(&db, master_secret);
    let query_server = QueryServer::new(&db);

    let offline_start = Instant::now();
    let mut client = Client::bootstrap_from_hint_server(
        config.client_id.clone(),
        config.n,
        config.m,
        config.active_hint_count,
        config.refresh_batch_size,
        &hint_server,
    );
    let offline_wall_ms = offline_start.elapsed().as_secs_f64() * 1000.0;

    let total_hints = config.active_hint_count + config.refresh_batch_size;
    let offline_client_outbound_bytes = config.client_id.len();
    let offline_client_inbound_bytes = total_hints * 2 * config.record_size + 32;

    let query_targets = build_query_targets(config.n * config.m, config.query_count);
    let online_start = Instant::now();
    let mut online_client_outbound_bytes = 0usize;
    let mut online_client_inbound_bytes = 0usize;
    let mut maintenance_client_outbound_bytes = 0usize;
    let mut maintenance_client_inbound_bytes = 0usize;
    let mut maintenance_wall_total_ms = 0.0f64;
    let mut refresh_count = 0usize;
    let mut refresh_hint_count = 0usize;

    for target in query_targets {
        let outcome = client
            .query_online_only(target, &query_server)
            .unwrap_or_else(|err| panic!("query for target {target} failed: {err}"));
        let expected = db.get_record(target).cloned().expect("target record missing");
        assert_eq!(outcome.target_record, expected, "benchmark recovery mismatch");

        online_client_outbound_bytes += compact_query_upload_bytes(config.n, config.m);
        online_client_inbound_bytes += 2 * config.record_size;

        let maintenance_start = Instant::now();
        let maintenance = client
            .run_maintenance(&hint_server)
            .unwrap_or_else(|err| panic!("maintenance after target {target} failed: {err}"));
        maintenance_wall_total_ms += maintenance_start.elapsed().as_secs_f64() * 1000.0;
        if maintenance.refresh_triggered {
            refresh_count += 1;
            refresh_hint_count += maintenance.fetched_hint_count;
            maintenance_client_outbound_bytes += config.client_id.len()
                + std::mem::size_of::<usize>() * 2;
            maintenance_client_inbound_bytes +=
                maintenance.fetched_hint_count * 2 * config.record_size;
        }
    }

    let online_wall_total_ms = online_start.elapsed().as_secs_f64() * 1000.0;
    let online_wall_avg_ms = online_wall_total_ms / config.query_count as f64;

    let online_network_total_ms =
        transmission_ms(online_client_outbound_bytes, config.bandwidth_mbps)
            + transmission_ms(online_client_inbound_bytes, config.bandwidth_mbps)
            + config.rtt_ms * config.query_count as f64;

    let online_end_to_end_total_ms = online_wall_total_ms + online_network_total_ms;
    let online_end_to_end_avg_ms = online_end_to_end_total_ms / config.query_count as f64;
    let maintenance_network_total_ms =
        transmission_ms(maintenance_client_outbound_bytes, config.bandwidth_mbps)
            + transmission_ms(maintenance_client_inbound_bytes, config.bandwidth_mbps)
            + config.rtt_ms * refresh_count as f64;
    let amortized_end_to_end_ms =
        (offline_wall_ms + online_end_to_end_total_ms + maintenance_wall_total_ms + maintenance_network_total_ms)
            / config.query_count as f64;

    AsympirexBenchmarkResult {
        scheme: "AsymPirex",
        trust_model: "two-server offline-online",
        total_records: db.total_records,
        partition_count: config.n,
        records_per_partition: config.m,
        record_size_bytes: config.record_size,
        query_count: config.query_count,
        active_hint_count: config.active_hint_count,
        refresh_batch_size: config.refresh_batch_size,
        network_bandwidth_mbps: config.bandwidth_mbps,
        network_rtt_ms: config.rtt_ms,
        offline_wall_ms,
        offline_client_outbound_bytes,
        offline_client_inbound_bytes,
        maintenance_wall_total_ms,
        maintenance_client_outbound_bytes,
        maintenance_client_inbound_bytes,
        refresh_count,
        refresh_hint_count,
        online_wall_total_ms,
        online_wall_avg_ms,
        online_network_total_ms,
        online_end_to_end_total_ms,
        online_end_to_end_avg_ms,
        online_client_outbound_bytes,
        online_client_inbound_bytes,
        amortized_end_to_end_ms,
        client_storage_bytes: estimate_client_storage_bytes(&client, config.record_size),
    }
}

fn build_query_targets(total_records: usize, query_count: usize) -> Vec<usize> {
    (0..query_count)
        .map(|idx| (idx * 7 + 5) % total_records)
        .collect()
}

fn compact_query_upload_bytes(n: usize, m: usize) -> usize {
    let offset_bits = n * ((usize::BITS - (m.saturating_sub(1)).leading_zeros()) as usize);
    let mask_bits = n;
    (offset_bits + mask_bits).div_ceil(8)
}

fn transmission_ms(total_bytes: usize, bandwidth_mbps: f64) -> f64 {
    let bits = total_bytes as f64 * 8.0;
    bits / (bandwidth_mbps * 1_000_000.0) * 1000.0
}

fn estimate_client_storage_bytes(client: &Client, record_size: usize) -> usize {
    let usize_bytes = std::mem::size_of::<usize>();
    let hint_id_bytes = std::mem::size_of::<usize>();

    let active_hint_bytes = client
        .active_hints
        .iter()
        .map(|(_, hint)| {
            hint_id_bytes
                + 2 * record_size
                + hint.patches.len() * 2 * usize_bytes
        })
        .sum::<usize>();

    let buffer_bytes = client
        .hbuffer
        .iter()
        .map(|packet| hint_id_bytes + 2 * packet.parities.rho_s.len())
        .sum::<usize>();

    let membership_bytes = client
        .table
        .table
        .iter()
        .map(|(_, hint_ids)| usize_bytes + hint_ids.len() * hint_id_bytes)
        .sum::<usize>();

    active_hint_bytes + buffer_bytes + membership_bytes
}
