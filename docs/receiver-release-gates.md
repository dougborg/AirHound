# External receiver validation and release gates

The MT7610U receiver remains experimental. Unit tests and short captures do not
establish an eight-hour soak, recovery reliability, calibrated RSSI, or parity
with the Linux mt76 driver. The tools below produce evidence for those gates;
creating a report does not itself complete them. Run commands against hardware
only when its owner has assigned the adapter to that test. Never let macOS,
a Linux VM, and another receiver process claim the same USB adapter at once.

## Reproducible soak

Build the pinned release executable and record the commit, dirty-tree status,
firmware SHA-256, executable SHA-256, OS version, hardware model, USB ID,
adapter/antenna revision, channel plan, and power-management configuration in a
separate local run manifest. Do not put serial numbers or captured network
identifiers in published artifacts. Use AC power and prevent sleep according to
the test machine's normal policy. A sleep/wake test is a separate recovery case.

From the repository root (substitute the locally verified firmware path):

```sh
mkdir -p drivers/mt7610u/target/validation
python3 scripts/receiver-soak.py \
  --output drivers/mt7610u/target/validation/soak-8h-new \
  --seconds 28800 --sample-seconds 5 --status-seconds 30 \
  --pause-every 300 --pause-seconds 2 \
  --max-rss-growth-mib 32 --max-fd-growth 0 --max-drops 0 \
  -- drivers/mt7610u/target/release/mt7610u-lab stream \
  --firmware /absolute/path/to/verified-firmware.bin \
  --channels 1,6,11,36,40,44,48 --dwell-ms 500 --max-seconds 32400
```

The child's nine-hour watchdog leaves room for an actual eight-hour interval
measured from the first `running` status. The harness owns completion and sends
SIGTERM after that interval. It requires a successful child exit and
`stopped.cleanup_verified=true`; early successful exit, truncated output,
unacknowledged commands, missing measurements, forced kill, or missing cleanup
confirmation fail. Shutdown time is unmeasured (`null`) when the child already
exited. A quiet radio with zero observations is valid for the lifecycle soak.
It is not evidence of useful reception.

The harness sends uniquely identified `status`, `pause`, and `resume` commands.
Only a matching `request_id` with the requested state acknowledges a command;
a periodic status cannot acknowledge it. A pause is checked for verified
cleanup. The run includes the short intentional pauses in its elapsed time;
it is not a claim of eight uninterrupted hours of reception. Requests too close
to the deadline to complete pause/resume are not scheduled. Recovery has a
bounded deadline and is counted separately from observation drops.

For an initial smoke test, use `--seconds 60 --sample-seconds 1`; retain the
nine-hour child watchdog. A one-second test can use `--seconds 1
--sample-seconds 0.1 --pause-every 0`; it checks basic command/shutdown behavior,
not stable resource consumption. Never rename a short report as eight-hour
validation.

`summary.json` is aggregate-only: elapsed time, acknowledgment count, child
CPU, RSS, descriptor counts, stream counters and failure codes. CPU is the
change in cumulative `ps` child CPU time divided by the measured sample span;
100% means one core. RSS comes from `ps`; descriptors come from procfs or
`lsof` (required on macOS). The initial and final RSS means each cover at most
five minutes. Sampling covers the direct child, not unrelated processes or
possible subprocess trees. Resource-query failures invalidate the measurement.

The output directory must be new and its parent must exist. The harness creates
it with mode 0700 and files with mode 0600. Raw stdout and stderr can contain
network identifiers and stay local. Each log defaults to four files of at most
1 MiB each (8 MiB combined); old segments rotate away. The aggregate report is
not rotated. Do not attach raw logs to a PR or upload them as CI artifacts.

Initial engineering acceptance budgets are no observation drops, no net file
descriptor growth, and no more than 32 MiB growth between initial/final RSS
windows. These are explicit resource budgets, not reception-performance claims.
Record any budget changes before running the test; investigate failures rather
than silently widening budgets. CPU is measured but has no portability-safe
hard threshold yet. A release gate requires a representative host CPU budget
agreed and recorded before the acceptance run.

## Controlled macOS versus Linux comparison

Use one adapter with the same antenna, orientation, physical position, supply,
fixed channel and HT20 bandwidth. Run sequentially without moving equipment,
using a controlled, stationary AP set. Record interference conditions and AP
configuration. Repeat both orders (macOS→Linux and Linux→macOS) at least three
times per test channel to expose time/order bias. A changing neighborhood scan
cannot establish receiver sensitivity or a fair beacon-recall benchmark.

The comparator measures **BSSID recall relative to Linux detections**, beacon
counts per equal observation window, and per-matched-BSSID mean signal deltas.
Linux detections are a reference, not exhaustive ground truth. macOS signal
values remain explicitly unvalidated EEPROM estimates. A positive signal delta
means the macOS estimate is numerically higher. Missing signal is excluded from
the signal comparison, never substituted with zero. Do not use a count from a
different channel, duration, antenna, or AP scene as a benchmark baseline.

Linux prerequisites: `iw`, `iproute2`, `wireshark-common`/`dumpcap`, `tshark`, and
`coreutils`. Before attaching USB, check the installed version's fields:

```sh
tshark --version
tshark -G fields | rg 'frame.time_epoch|wlan.bssid|radiotap.channel.freq|radiotap.dbm_antsignal|radiotap.flags.badfcs'
```

Provisioning packages and USB passthrough are separate operator actions. A VM
must expose the physical USB device to the Linux guest; a normal Docker network
interface cannot stand in for a USB monitor-mode receiver. Confirm Linux uses
`mt76x0u` (`ethtool -i` or sysfs), record the kernel/firmware versions, and record
`iw phy` capabilities. With the adapter assigned exclusively to Linux, select
the actual interface/PHY from `iw dev`. A representative fixed-channel setup
is below; replace interface names with the verified names. These commands
change the guest's adapter configuration and must be run by the hardware owner.

```sh
ip link set wlan0 down
iw dev wlan0 set type monitor
ip link set wlan0 up
iw dev wlan0 set channel 36 HT20
iw dev wlan0 info
umask 077
dumpcap -i wlan0 -a duration:60 -w /private/local/run/linux.pcapng
```

Do not assume a successful `dumpcap` invocation proves the intended duration
or channel. Record actual monotonic capture start/end, tool exit status and
reported kernel/capture drops in the private run manifest. Confirm radiotap
frequency is present and matches the selected channel. Exclude runs with
capture drops from the acceptance comparison. The tool does not infer these
facts from packet timestamps: a quiet capture has a short packet span despite a
long recording interval.

Capture macOS observations with the existing fixed-channel `capture` command
and its explicit duration/output options. The comparison input is a complete
beacon-v1 observation file, not the mixed control/observation `stream` output.
Use equivalent windows wholly within both measured acquisition intervals;
record their Unix start/end timestamps from the controlled run. Host clocks
must be checked, but sequential windows need not overlap. Use the same channel
for both captures and no hopping. Beacons observed off their advertised channel
are not silently relabeled; comparison groups by configured receive channel.

Export only beacon frames without a radiotap bad-FCS flag. Missing bad-FCS
metadata is not proof of a valid FCS; record the Linux driver's filter policy.
The receiver's beacon parser already excludes decoded CRC-error frames.

```sh
umask 077
tshark -r /private/local/run/linux.pcapng \
  -Y 'wlan.fc.type_subtype == 0x08 && !(radiotap.flags.badfcs == 1)' \
  -T fields -E occurrence=f \
  -e frame.time_epoch -e wlan.bssid \
  -e radiotap.channel.freq -e radiotap.dbm_antsignal \
  > /private/local/run/linux-beacons.tsv
```

This TSV uses the default tab separator, no header, and the first reported
antenna signal. The comparator rejects missing radiotap frequency, malformed
records, unsupported channels, input over 64 MiB, and lines over 16 KiB. It does
not require installing a Python PCAP library and does not invoke hardware or
`tshark` itself. If `tshark` is unavailable, install it before export; do not
substitute an Ethernet capture that lacks receive-channel metadata.

Create a private `controls.json`. These timestamps are illustrative and **must
be replaced** with the actual equal-duration measured windows. All booleans are
operator attestations; the tool cannot verify antenna placement or AP stability.

```json
{
  "schema_version": 1,
  "channel": 36,
  "bandwidth_mhz": 20,
  "mac_window_unix_s": [1000, 1060],
  "linux_window_unix_s": [2000, 2060],
  "same_adapter": true,
  "same_antenna": true,
  "same_position": true,
  "same_capture_filter": true,
  "controlled_ap_set": true,
  "stationary_scene": true,
  "no_channel_hopping": true
}
```

```sh
python3 scripts/receiver-compare.py \
  --mac /private/local/run/mac-beacons.ndjson \
  --linux-tsv /private/local/run/linux-beacons.tsv \
  --controls /private/local/run/controls.json \
  > /private/local/run/comparison-summary.json
```

For an ambient AP scene, set `controlled_ap_set` to `false` and add
`--exploratory`. All other attestations remain mandatory. This mode labels the
report `comparison_kind: exploratory`, `conclusive: false`, and
`validation_passed: false` (exit 1 even with nonempty captures), with an
explicit warning that temporal variation confounds recall and signal deltas.
Use macOS→Linux→macOS windows of at least 30 seconds each as an initial
exploration; do not treat their counts as a controlled reception benchmark.

A successful exit means the comparison was valid and both windows contained
beacons. It is **not a performance gate pass**. An empty side is inconclusive
(exit 1), with undefined recall/deltas represented as `null`; invalid inputs
exit 2. Choose reception acceptance thresholds using repeated controlled
baselines and confidence intervals before evaluating a release. Do not invent
thresholds from the first favorable capture. Publish only aggregate summaries
and non-identifying setup information; PCAP, TSV and observation files stay
private.

## Release evidence matrix

| Gate | Required evidence | Current status |
| --- | --- | --- |
| Hardware-free correctness | Rust tests/Clippy, Python harness/comparator tests, protocol fixture validation | Runnable; record exact commit and check results per release |
| Eight-hour operation | Actual 28,800 seconds after readiness, bounded RSS/FDs, no drops, CPU budget, verified SIGTERM cleanup | Not established by short captures or mocked tests |
| Pause/resume | Repeated acknowledged transitions, no observations after confirmed pause, fresh session/sequence after resume | Requires real-device acceptance run |
| Disconnect/reconnect | Unplug during running/paused, bounded recovery, no stale epoch/session observations, explicit failure after recovery budget | Requires controlled hardware fault run |
| Failure cleanup | SIGTERM, stdin EOF, broken stdout, stalled consumer, firmware failure, busy USB, interrupted initialization | Unit tests plus hardware cases required; do not assert register cleanup after a physically removed device |
| macOS/Linux reception | Equal controlled windows on each supported band, repeated order, drop-free captures, relative recall and signal-delta report | Not established without controlled captures |
| Hardware/host coverage | Adapter USB ID/board revision, macOS Apple Silicon and Intel if supported, Linux native or USB-passthrough kernel/mt76 versions, antenna configuration | Only tested combinations may be claimed; untested combinations remain experimental |
| Deployment | Reproducible package hashes/firmware provenance, privilege/ownership checks, versioned process protocol, rollback/uninstall recipe | Separate packaging/integration evidence required |

For sleep/wake, unplug, stdout stalls and recovery exhaustion, record the
injected event time and expected outcome before the run. The standard soak
harness does not itself unplug USB, stop VMs, suspend the machine, or stall its
own reader. Keep those fault procedures explicit and supervised. A process
exiting cleanly does not prove the adapter is still receiving; combine lifecycle
evidence with a controlled AP fixture or an independent reference.

Hardware-free checks (these launch only synthetic Python children):

```sh
python3 -B -m unittest discover -s scripts/tests -p 'test_receiver_*.py' -v
```

## Local Linux inventory

The development host has UTM and OrbStack installed. The default sandbox could
not enumerate UTM guests or query the OrbStack socket. A separately authorized
inventory found a Debian guest running Linux 6.1.0-30-arm64. This establishes a
possible USB-passthrough test environment, not working adapter passthrough,
mt76 binding, or controlled benchmark results. A subsequent authorized check
verified `iw`, `tshark`, `dumpcap`, and the guest `mt76x0u` kernel module; firmware
installation and physical USB assignment remain separate actions. Record
those checks in each actual run rather than treating the inventory as validation.
