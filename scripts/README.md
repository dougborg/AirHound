# Development and validation tools

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Testing guide](../docs/testing.md)

Run these tools from the repository root. Prefer the `just` recipes where listed;
use `python3 scripts/<tool>.py --help` for script options. Live tools require the
relevant hardware and permissions. Keep captures, identifiers and flash backups
out of commits and CI artifacts.

| Task | Tool / entry point | Documentation |
| --- | --- | --- |
| Generate shared signature tables | `just generate-signatures`, `just check-signatures` | [Pack authoring](../docs/signature-packs.md) |
| Package macOS permission bundle | `just bundle-hostd`, `just bundle-hostd-public-api` | [Desktop setup](../hostd/README.md#build-and-run) |
| Benchmark desktop processing without radios | `just bench-hostd` | [Methodology and results](../hostd/README.md#tests-and-performance) |
| Profile live macOS capture | `just profile-hostd` | [Permissions and measurement limits](../hostd/README.md#tests-and-performance) |
| Monitor firmware serial output | [monitor-firmware.py](monitor-firmware.py) | [Bring-up](../docs/m5stickc-bringup.md), [streaming](../docs/firmware-streaming.md) |
| Check linked firmware memory layout | [check-firmware-memory.py](check-firmware-memory.py) | [Firmware memory/stack limits](../docs/m5-stream-performance.md) |
| Capture passive Sniffle BLE packets | [sniffle-source.py](sniffle-source.py), separate GPL-3.0-only helper | [Live and replay source](../docs/sniffle-source.md) |
| Replay saved BLE HCI data | [replay-hci-ble.py](replay-hci-ble.py) | [USB HCI investigation](../docs/usb-hci-receiver-investigation.md) |
| Run receiver soak / compare captures | [receiver-soak.py](receiver-soak.py), [receiver-compare.py](receiver-compare.py) | [Receiver testing](../drivers/mt7610u/TESTING.md) |
| Package a desktop binary and notices | [package-desktop.py](package-desktop.py) | [Release inventory](../docs/release-status.md) |
| Verify complete release asset set | [check-release-assets.py](check-release-assets.py) | [Release inventory](../docs/release-status.md) |
| Package an independent receiver candidate | [package-receiver.py](package-receiver.py) | [Release gates](../docs/receiver-release-gates.md) |

Synthetic benchmarks measure parsing, matching and output throughput; they do
not measure RF scan frequency. Linked firmware memory checks do not measure
runtime stack high-water. Offline replay cannot establish live adapter support.

## Checks

```sh
python3 -B -m unittest discover -s scripts/tests -v
node --test .github/scripts/preview-comment.test.js
```

Signature compiler tests require `jsonschema` (installed with `check-jsonschema`):

```sh
python3 -m unittest discover -s signatures/tests -v
```

Coverage orchestration lives in [`.github/scripts/coverage.sh`](../.github/scripts/coverage.sh),
with floors in [coverage-thresholds.json](../.github/coverage-thresholds.json).
See the [testing guide](../docs/testing.md) for pinned toolchains, fuzzing and CI.
Hardware procedures remain explicit manual validation; do not infer a release
from a successful script exit or a short capture alone.
