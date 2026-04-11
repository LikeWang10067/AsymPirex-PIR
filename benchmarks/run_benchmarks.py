from __future__ import annotations

import argparse
from pathlib import Path

from runners.asympirex import run_asympirex
from runners.common import BenchmarkCase, UnifiedResult, load_config, write_raw_result, write_summary
from runners.piano import run_piano
from runners.pirex import run_pirex


RUNNERS = {
    "asympirex": run_asympirex,
    "piano": run_piano,
    "pirex": run_pirex,
}


def main() -> None:
    parser = argparse.ArgumentParser(description="Run cross-PIR benchmarks.")
    parser.add_argument("--config", required=True, help="Path to benchmark config JSON.")
    args = parser.parse_args()

    config_path = Path(args.config).resolve()
    config, cases, network = load_config(config_path)
    repetitions = int(config.get("repetitions", 1))

    results: list[UnifiedResult] = []
    for case in cases:
        for scheme in case.schemes:
            runner = RUNNERS[scheme]
            result = runner(case, network, repetitions)
            write_raw_result(result)
            results.append(result)

    write_summary(results)


if __name__ == "__main__":
    main()
