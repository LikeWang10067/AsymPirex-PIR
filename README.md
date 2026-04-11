# AsymPirex-PIR

This repository contains:

- a Rust prototype of `AsymPirex`
- a reproducible benchmark harness for comparing `AsymPirex`, `Pirex`, and `Piano`
- paper-oriented evaluation artifacts and output summaries

The main code for our scheme lives under `asympirex/`, and the experiment runner lives under `benchmarks/`.

## Repository Layout

- `asympirex/`: Rust implementation of the AsymPirex prototype
- `benchmarks/`: cross-scheme benchmark harness and configs
- `paper/`: paper text and evaluation write-up

## Source Code Structure

The core AsymPirex implementation is in `asympirex/src/`.

- `asympirex/src/database.rs`
  - In-memory database model.
  - Stores the logical database as `n` partitions with `m` records each.
  - Provides `get_record()` and `set_record()`.

- `asympirex/src/crypto.rs`
  - PRF/PRG utilities.
  - Derives the client secret key `usk`, per-hint seeds, and expands each hint into per-partition offsets and masks.
  - This is where the pseudorandom structure of hints is generated.

- `asympirex/src/server.rs`
  - Implements the two logical servers:
    - `HintServer`: handles bootstrap and batched refresh
    - `QueryServer`: handles online patched parity queries
  - Defines the request/response message structs used by the prototype.

- `asympirex/src/client.rs`
  - Implements the AsymPirex client state and query logic.
  - Maintains:
    - the active hint set
    - the buffered hint queue
    - the local hint-membership table
  - Contains:
    - `query_online_only()`: one-server online query path
    - `run_maintenance()`: batched refresh path with the Hint Server
    - local logical index replacement and hint state updates after each query

- `asympirex/src/utils.rs`
  - Small XOR helpers used throughout the prototype.

- `asympirex/src/pirex_reimpl.rs`
  - Local Rust reconstruction of base Pirex used for fair comparison in the same benchmark framework.

- `asympirex/src/bin/asympirex_bench.rs`
  - Benchmark entry point for AsymPirex.
  - Runs bootstrap, online queries, maintenance, and writes JSON results.

- `asympirex/src/bin/pirex_reimpl_bench.rs`
  - Benchmark entry point for the reconstructed Pirex baseline.

- `asympirex/src/main.rs`
  - End-to-end demo / trace driver for the AsymPirex prototype.
  - Writes detailed logs to `asympirex/output/`.

## What To Install

Run everything from the repository root.

Required tools:

- `python3`
- `cargo` and Rust toolchain
- `go`

Required Python package:

```bash
python3 -m pip install psutil
```

## Build The Rust Code

To compile the Rust project:

```bash
cargo build --manifest-path asympirex/Cargo.toml
```

To run tests:

```bash
cargo test --manifest-path asympirex/Cargo.toml
```

## Run The AsymPirex Prototype

To run the standalone AsymPirex prototype and generate a local client trace:

```bash
cargo run --manifest-path asympirex/Cargo.toml
```

This writes:

- `asympirex/output/client_trace.json`
- `asympirex/output/client_run.log`

These files are useful if you want to inspect the client's tables before and after queries.

## Run Experiments

The benchmark harness entry point is:

```bash
python3 benchmarks/run_benchmarks.py --config <config.json>
```

### Quick Sanity Check

Run the smallest shared pilot:

```bash
python3 benchmarks/run_benchmarks.py --config benchmarks/config/pilot.json
```

This compares:

- `AsymPirex`
- `Pirex`
- `Piano`

on a very small configuration for fast validation.

### Reproduce The Paper Evaluation

Run:

```bash
python3 benchmarks/run_benchmarks.py --config benchmarks/config/paper_eval.json
```

This runs the paper-oriented benchmark matrix currently used in this repository.

## Benchmark Config Files

The benchmark configs are stored in `benchmarks/config/`.

- `benchmarks/config/pilot.json`
  - small sanity-check run

- `benchmarks/config/paper_eval.json`
  - paper-oriented evaluation matrix

Each config specifies:

- the network model
- one or more benchmark cases
- the schemes to run

Important fields:

- `logical_db_size`: number of logical records
- `record_size_bytes`: record size in bytes
- `query_count`: number of online queries
- `active_hint_count`: number of active hints for AsymPirex
- `refresh_batch_size`: batched refresh size for AsymPirex
- `schemes`: subset of `asympirex`, `pirex`, `piano`

## Where Results Go

After a run, outputs are written to:

- `benchmarks/results/raw/`
  - per-run JSON files

- `benchmarks/results/summary/raw.json`
  - merged JSON summary

- `benchmarks/results/summary/summary.csv`
  - merged CSV summary for tables/plots

The most useful file for paper tables is usually:

```bash
benchmarks/results/summary/summary.csv
```

Important columns include:

- `query_only_avg_ms`
- `amortized_ms`
- `client_outbound_bytes`
- `client_inbound_bytes`
- `client_storage_bytes`
- `maintenance_total_ms`
- `refresh_count`

## Scheme Notes

### AsymPirex

- Implemented in Rust in this repository.
- Online and maintenance costs are tracked separately.
- Network cost is computed analytically from measured message sizes under the configured bandwidth and RTT.

### Pirex

- Uses a local Rust reconstruction in `asympirex/src/pirex_reimpl.rs`.
- This avoids the original artifact's platform-specific build issues and keeps the comparison reproducible.

### Piano

- Uses the vendored Go artifact in `benchmarks/external/Piano-PIR-new/`.
- A local TCP relay is used to emulate the configured network profile.
- Some larger cases in the public artifact may crash internally; such failures are recorded in the output JSON/CSV rather than silently ignored.

## Custom Experiments

To define a new experiment:

1. Copy one of the JSON files in `benchmarks/config/`
2. Edit the fields you want
3. Run it with `benchmarks/run_benchmarks.py`

Example:

```bash
python3 benchmarks/run_benchmarks.py --config benchmarks/config/my_experiment.json
```

## Troubleshooting

- If `psutil` is missing, install it with `python3 -m pip install psutil`.
- If `go` is missing, `Piano` runs will fail, but `AsymPirex` and `Pirex` can still run.
- If `cargo` is missing, both `AsymPirex` and `Pirex` runs will fail.
- If a run fails, inspect the corresponding file in `benchmarks/results/raw/` for the captured error message.
- If you only want to benchmark `AsymPirex`, remove the other schemes from the config file.

## Additional Notes

- A more benchmark-specific guide is available in `benchmarks/README.md`.
- The paper evaluation write-up currently lives in `paper/performance_evaluation.tex`.
