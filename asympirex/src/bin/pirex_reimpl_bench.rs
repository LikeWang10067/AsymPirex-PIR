use asympirex::pirex_reimpl::run_reimplemented_pirex_benchmark;
use serde::Serialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    total_records: usize,
    record_size: usize,
    query_count: usize,
    bandwidth_mbps: f64,
    rtt_ms: f64,
    output_path: PathBuf,
}

#[derive(Serialize)]
struct PirexBenchmarkResult {
    scheme: &'static str,
    trust_model: &'static str,
    topology: &'static str,
    total_records: usize,
    record_size_bytes: usize,
    query_count: usize,
    exponent: usize,
    group_log: usize,
    group_size: usize,
    group_count: usize,
    hint_count: usize,
    network_bandwidth_mbps: f64,
    network_rtt_ms: f64,
    offline_client_compute_ms: f64,
    offline_server_compute_ms: f64,
    offline_ms: f64,
    offline_client_outbound_bytes: usize,
    offline_client_inbound_bytes: usize,
    online_wall_total_ms: f64,
    online_client_compute_total_ms: f64,
    online_client_compute_avg_ms: f64,
    online_server_compute_total_ms: f64,
    online_server_compute_avg_ms: f64,
    online_network_total_ms: f64,
    online_end_to_end_total_ms: f64,
    online_end_to_end_avg_ms: f64,
    online_client_outbound_bytes: usize,
    online_client_inbound_bytes: usize,
    amortized_ms: f64,
    client_storage_bytes: usize,
}

fn main() {
    let config = parse_args();
    let result = run_benchmark(&config).unwrap_or_else(|err| panic!("pirex benchmark failed: {err}"));

    if let Some(parent) = config.output_path.parent() {
        fs::create_dir_all(parent).expect("failed to create benchmark output directory");
    }

    let json = serde_json::to_string_pretty(&result).expect("failed to serialize benchmark result");
    fs::write(&config.output_path, json).expect("failed to write benchmark result");
}

fn parse_args() -> BenchmarkConfig {
    let mut total_records = 32usize;
    let mut record_size = 64usize;
    let mut query_count = 20usize;
    let mut bandwidth_mbps = 40.0f64;
    let mut rtt_ms = 60.0f64;
    let mut output_path = PathBuf::from("asympirex/output/pirex_reimpl_result.json");

    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("missing value for {flag}"));
        match flag.as_str() {
            "--total-records" => total_records = value.parse().expect("invalid --total-records"),
            "--record-size" => record_size = value.parse().expect("invalid --record-size"),
            "--query-count" => query_count = value.parse().expect("invalid --query-count"),
            "--bandwidth-mbps" => bandwidth_mbps = value.parse().expect("invalid --bandwidth-mbps"),
            "--rtt-ms" => rtt_ms = value.parse().expect("invalid --rtt-ms"),
            "--output" => output_path = PathBuf::from(value),
            other => panic!("unknown flag {other}"),
        }
    }

    BenchmarkConfig {
        total_records,
        record_size,
        query_count,
        bandwidth_mbps,
        rtt_ms,
        output_path,
    }
}

fn run_benchmark(config: &BenchmarkConfig) -> Result<PirexBenchmarkResult, String> {
    let metrics = run_reimplemented_pirex_benchmark(
        config.total_records,
        config.record_size,
        config.query_count,
    )?;

    let online_network_total_ms =
        transmission_ms(metrics.online.client_outbound_bytes, config.bandwidth_mbps)
            + transmission_ms(metrics.online.client_inbound_bytes, config.bandwidth_mbps)
            + config.rtt_ms * config.query_count as f64;
    let online_end_to_end_total_ms = metrics.online.wall_total_ms + online_network_total_ms;
    let online_end_to_end_avg_ms = online_end_to_end_total_ms / config.query_count as f64;
    let offline_ms = metrics
        .offline
        .client_compute_ms
        .max(metrics.offline.server_compute_ms);
    let amortized_ms = (offline_ms + online_end_to_end_total_ms) / config.query_count as f64;

    Ok(PirexBenchmarkResult {
        scheme: "Pirex",
        trust_model: "two-server offline-online",
        topology: "in-process Rust reconstruction with analytical network",
        total_records: metrics.params.total_records,
        record_size_bytes: metrics.params.record_size,
        query_count: metrics.query_count,
        exponent: metrics.params.exponent,
        group_log: metrics.params.group_log,
        group_size: metrics.params.group_size,
        group_count: metrics.params.group_count,
        hint_count: metrics.params.hint_count,
        network_bandwidth_mbps: config.bandwidth_mbps,
        network_rtt_ms: config.rtt_ms,
        offline_client_compute_ms: metrics.offline.client_compute_ms,
        offline_server_compute_ms: metrics.offline.server_compute_ms,
        offline_ms,
        offline_client_outbound_bytes: metrics.offline.client_outbound_bytes,
        offline_client_inbound_bytes: metrics.offline.client_inbound_bytes,
        online_wall_total_ms: metrics.online.wall_total_ms,
        online_client_compute_total_ms: metrics.online.client_compute_total_ms,
        online_client_compute_avg_ms: metrics.online.client_compute_avg_ms,
        online_server_compute_total_ms: metrics.online.server_compute_total_ms,
        online_server_compute_avg_ms: metrics.online.server_compute_avg_ms,
        online_network_total_ms,
        online_end_to_end_total_ms,
        online_end_to_end_avg_ms,
        online_client_outbound_bytes: metrics.online.client_outbound_bytes,
        online_client_inbound_bytes: metrics.online.client_inbound_bytes,
        amortized_ms,
        client_storage_bytes: metrics.client_storage_bytes,
    })
}

fn transmission_ms(total_bytes: usize, bandwidth_mbps: f64) -> f64 {
    let bits = total_bytes as f64 * 8.0;
    bits / (bandwidth_mbps * 1_000_000.0) * 1000.0
}
