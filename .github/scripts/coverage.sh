#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
suite=${1:?expected core or hostd}
case "$suite" in
  core) args=(--manifest-path Cargo.toml --lib --no-default-features) ;;
  hostd) args=(--manifest-path hostd/Cargo.toml) ;;
  *) echo "Unknown coverage suite: $suite" >&2; exit 2 ;;
esac
# Keep coverage binaries separate from ordinary tests and other Rust versions.
export CARGO_LLVM_COV_TARGET_DIR="$PWD/target/coverage-$suite"
report_dir="$PWD/coverage/$suite"
mkdir -p "$report_dir"
floor=$(python3 -c 'import json,sys; key = sys.argv[1] if sys.argv[1] == "core" else "hostd-" + sys.platform; print(json.load(open(".github/coverage-thresholds.json"))[key])' "$suite")
status=0
cargo llvm-cov "${args[@]}" --locked --json --summary-only \
  --fail-under-lines "$floor" --output-path "$report_dir/summary.json" || status=$?
cargo llvm-cov report --manifest-path "${args[1]}" --html --output-dir "$report_dir/html"
python3 - "$report_dir/summary.json" "$suite" "$floor" <<'PY'
import json, os, sys
report, suite, floor = sys.argv[1:]
with open(report) as f:
    totals = json.load(f)["data"][0]["totals"]
if not totals["lines"]["count"]:
    raise SystemExit("No instrumented lines: coverage report is invalid")
summary = f"### {suite}: production-code coverage\n\n"
summary += "| Metric | Covered | Total | Percent |\n|---|---:|---:|---:|\n"
for metric in ("lines", "functions", "regions"):
    row = totals[metric]
    summary += f"| {metric} | {row['covered']} | {row['count']} | {row['percent']:.2f}% |\n"
summary += f"\nRequired line coverage: {floor}%. Test bodies are excluded. Hardware paths are not exercised.\n"
print(summary)
if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
        f.write(summary)
PY
exit "$status"
