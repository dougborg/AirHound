#!/bin/sh
set -eu
cd "$(dirname "$0")"
benchmark_dir=$(mktemp -d "${TMPDIR:-/tmp}/airhound-table-benchmark.XXXXXX")
trap 'rm -rf "$benchmark_dir"' EXIT
swiftc -O -parse-as-library -emit-library -emit-module -module-name AirHoundCore \
  -emit-module-path "$benchmark_dir/AirHoundCore.swiftmodule" Sources/AirHoundCore/*.swift \
  -o "$benchmark_dir/libAirHoundCore.dylib"
swiftc -O -parse-as-library -I "$benchmark_dir" -L "$benchmark_dir" -lAirHoundCore \
  -Xlinker -rpath -Xlinker "$benchmark_dir" Sources/AirHoundApp/ObservationTable.swift \
  Benchmarks/TableBenchmark.swift -o "$benchmark_dir/table-benchmark"
"$benchmark_dir/table-benchmark"
