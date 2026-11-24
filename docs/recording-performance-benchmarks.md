# Private recording performance measurements

These tools measure an existing consistent recording snapshot without opening a service or any receivers. They are informational benchmarks, not CI correctness tests: no elapsed-time threshold determines success. Data bounds, cursor order, repeated page equality, held selection and viewport behavior are checked separately.

Prepare a consistent SQLite backup using the application's export or the SQLite backup API. Do not copy a live database file without its transaction state. Check free disk space first. Supply the snapshot explicitly; neither tool defaults to the installed database. The tools never call `Store::open` or reconcile saved run state.

Run the real Rust reader in release mode, selecting a stored run or defaulting to the newest:

```sh
cargo run --release --locked --manifest-path hostd/Cargo.toml \
  --example recording_benchmark -- \
  --snapshot /path/to/private-snapshot.sqlite \
  --output /path/to/new-private-benchmark-directory \
  --export-pages
```

`--run RUN_ID` selects another recording. Omit `--export-pages` to keep raw page data out of the output. An existing output directory is refused. Output directories/files use private permissions. Do not commit or upload the snapshot or exported pages: they contain the user's observations. Keep source revision, toolchain and snapshot digest alongside a report when reproducibility matters.

The reader samples three cursor regions, up to eight pages each, three times. Small runs can make those regions overlap. Each page requests at most 500 records and must fit the existing 512 KiB response limit. Timing includes SQLite selection, structured payload decoding, normalized metadata projection and internal serialization used for page accounting. Response serialization is measured separately; neither measurement includes IPC, command-process startup or GUI work. Build mode is recorded. Operating-system caches are not controlled, and later samples warm them; report individual samples rather than claiming cold-cache results.

On macOS, replay the exported first region into the production presentation components:

```sh
sh apps/macos/benchmark-snapshot.sh /path/to/private-benchmark-directory
```

This compiles optimized Swift and creates only its own offscreen table. It measures page decoding, append/normalization, cached-window access, held-table updates, Follow live, filtering/sorting, exploratory grouping and 300 layout/display scroll steps. Grouping is an experiment over reported protocol/receiver/address, not a shipped grouping feature or proof of persistent physical-device identity. It reports whether the provided rows actually exercised rolling-window eviction; small samples must not be presented as eviction coverage.

The presentation result refuses overwrite. The operational 120-second outer guard only terminates a hung process. This is not a user-perceived frame-rate or end-to-end installed-app latency measurement. In production, record normalization already runs in a detached task and backend page decoding is in a nonisolated async client method; synchronous benchmark timings must not be described as measured main-thread stalls. Use a profiler and an executor witness before drawing that conclusion.

Keep private-data measurements outside source control. Portable correctness tests should continue using deterministic synthetic fixtures; benchmark wall time is intentionally physical and separately labeled under the test timing policy (#329).
