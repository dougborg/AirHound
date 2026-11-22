#!/bin/sh
# Synthetic presentation microbenchmark; no backend, recording, or radio access.
set -eu
cd "$(dirname "$0")"
benchmark_dir=$(mktemp -d "${TMPDIR:-/tmp}/airhound-observation-bench.XXXXXX")
trap 'rm -rf "$benchmark_dir"' EXIT HUP INT TERM
swiftc -O Sources/AirHoundCore/*.swift Benchmarks/ObservationBenchmark.swift -o "$benchmark_dir/benchmark"
"$benchmark_dir/benchmark"
