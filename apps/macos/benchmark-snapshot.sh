#!/bin/sh
# Informational offscreen measurement using private pages exported by recording_benchmark.
set -eu
if [ "$#" -ne 1 ]; then
  echo 'usage: benchmark-snapshot.sh PRIVATE_READER_OUTPUT_DIRECTORY' >&2
  exit 2
fi
pages_dir=$(cd "$1" && pwd)
cd "$(dirname "$0")"
umask 077
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/airhound-snapshot-benchmark.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM
swiftc -O -parse-as-library -emit-library -emit-module -module-name AirHoundCore \
  -emit-module-path "$build_dir/AirHoundCore.swiftmodule" Sources/AirHoundCore/*.swift \
  -o "$build_dir/libAirHoundCore.dylib"
swiftc -O -parse-as-library -I "$build_dir" -L "$build_dir" -lAirHoundCore \
  -Xlinker -rpath -Xlinker "$build_dir" Sources/AirHoundApp/ObservationTable.swift \
  Sources/AirHoundApp/ObservationPresentation.swift Benchmarks/SnapshotPresentationBenchmark.swift \
  -o "$build_dir/snapshot-benchmark"
python3 - "$build_dir/snapshot-benchmark" "$pages_dir" <<'PY'
# Operational hang guard only; success requires the benchmark's actual exit.
import subprocess, sys
subprocess.run(sys.argv[1:], check=True, timeout=120)
PY
