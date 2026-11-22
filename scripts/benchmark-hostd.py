#!/usr/bin/env python3
"""Run the release-only, hardware-free desktop benchmark and save JSON results."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'hostd/target/performance/benchmark.json')
    parser.add_argument('--no-default-features', action='store_true', help='Measure the build excluding private macOS APIs')
    args = parser.parse_args()
    invocation = ['cargo', 'test', '--manifest-path', 'hostd/Cargo.toml', '--locked', '--release']
    if args.no_default_features:
        invocation.append('--no-default-features')
    invocation += ['performance_event_pipeline', '--', '--ignored', '--nocapture']
    run = subprocess.run(invocation, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(run.stdout, end='')
    if run.returncode:
        return run.returncode
    samples = []
    for line in run.stdout.splitlines():
        if line.startswith('PERF '):
            fields = dict(re.findall(r'(\w+)=(\S+)', line))
            samples.append({key: value if key == 'scenario' else float(value) if key == 'elapsed_s' else int(value) for key, value in fields.items()})
    if len(samples) != 6:
        raise RuntimeError(f'Expected six measured batches, received {len(samples)}')
    summaries = {}
    for scenario in sorted({sample['scenario'] for sample in samples}):
        rates = [sample['events_per_s'] for sample in samples if sample['scenario'] == scenario]
        if len(rates) != 3:
            raise RuntimeError(f'Incomplete scenario: {scenario}')
        summaries[scenario] = {'median_events_per_s': statistics.median(rates), 'min_events_per_s': min(rates), 'max_events_per_s': max(rates)}
    report = {
        'schema_version': 2,
        'timestamp_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'git_commit': command('git', 'rev-parse', 'HEAD'),
        'working_tree_dirty': bool(command('git', 'status', '--porcelain')),
        'os': platform.platform(), 'architecture': platform.machine(), 'logical_cpus': os.cpu_count(),
        'rustc': command('rustc', '--version'), 'default_features': not args.no_default_features,
        'method': {'events_per_batch': 100000, 'ble_fraction': 0.2, 'tokio_workers': 2, 'fsync': False, 'live_radios': False, 'timing_gate': False},
        'samples': samples, 'summaries': summaries,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f'Report: {args.output}')
    summary = '\n'.join(f'- {name}: {values["median_events_per_s"]:,.0f} events/s median' for name, values in summaries.items())
    print(summary)
    if os.getenv('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
            stream.write('### Desktop event pipeline benchmark\n\n' + summary + '\n\nInformational timing; synthetic input, cached writes, no live radios or fsync.\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
