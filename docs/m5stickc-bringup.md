# M5StickC Plus2 bring-up

The connected test board was identified through the ESP ROM loader as an
ESP32-PICO-V3-02 revision 3.1 with 8 MB flash and embedded PSRAM. M5Stack's
[Plus2 specifications](https://docs.m5stack.com/en/core/StickC-Plus2) list 2 MB
PSRAM and the red LED/IR emitter on GPIO19. These are hardware capabilities;
the current no_std firmware uses PSRAM for observations and keeps radio/RTOS
allocations in an internal heap. Earlier results below record the bring-up
stages before PSRAM was enabled.

The existing firmware identified itself as AirHound v0.1.0. A short serial
observation showed WiFi initialization, BLE scanning/advertising,
display initialization and buzzer initialization without a panic.

## Backup and build provenance

Before updating a board, save the entire flash and verify it against the device.
A flash backup may contain settings and credentials: keep it private and out
of Git. Local bring-up artifacts live under ignored `target/hardware/` with
private backup and raw-log files. The manifest records source revision, dirty
state, source patch, build image, binary sizes/hashes, memory sections, and
verification/update results.

For this CH9102 serial adapter on macOS, chip identification worked normally,
but flash reads at 921600 and 460800 baud failed with serial corruption.
The read was retried at 115200 baud. Do not flash until a complete 8 MB backup
has passed device-side verification.

Build the locked `m5stickc` no_std target with the repository's pinned ESP Rust
image. Generate an image for the detected 8 MB flash using `espflash save-image`.
Use the image's partition table to locate its segments; do not assume offsets
for a different board or build. The current image uses bootloader 0x1000,
partition table 0x8000, NVS 0x9000, PHY data 0xf000, and application 0x10000.
Flashing the bootloader, partition table and application separately preserves
the NVS/PHY region. The full original flash backup remains the rollback source.

## Bounded runtime observation

Install `pyserial` in a local virtual environment. The monitor observes serial
output only; it neither flashes nor sends application commands. Opening a
serial port can reset this adapter/board, so begin a fresh observation after
flashing. From the repository root:

```sh
python scripts/monitor-firmware.py \
  --port /dev/cu.usbserial-DEVICE \
  --output target/hardware/new-m5stick-observation \
  --seconds 180 --board m5stickc_plus2 --version 0.1.1
```

The output directory must be new. Raw serial logs are private and bounded to
64 MiB; summaries contain message counts and status metrics, not wireless
identifiers. The monitor requires at least two status samples from the expected
board/version, with scanning enabled and nondecreasing uptime. Panics, repeated
startup banners, invalid metrics, or serial failures make the check fail.
It reports minimum/maximum free heap without claiming a long-term leak result.

Parser regression tests run in the existing Python CI test suite. Firmware
startup and heap measurements require real hardware. Display appearance,
audible buzzer behavior, battery operation, RF sensitivity and long-duration
stability require separate checks; initialization logs alone do not validate
those properties.

The display's WiFi/BLE counters count signature matches, not every received
packet or unique device. Periodic `WiFi stats` serial lines separately report
received packets, parsed events, queue drops, channel-hop failures, and matches.
The monitor includes these aggregates in `wifi_stats`; a nonzero hop-error
count fails the observation. A passing monitor result alone does not require
RF traffic, so inspect rising receive/parsed counters to confirm reception.

WiFi initialization must set station mode and await `start_async()` before
enabling promiscuous reception, as in the pinned esp-radio sniffer example.
Starting the interface does not connect to an AP. The initial v0.1.1 bring-up
below omitted this step: its successful initialization logs did not establish
working WiFi. Channel changes previously also discarded driver errors.

## Local update result, 2026-11-19

The full 8,388,608-byte backup completed at 115200 baud and passed device-side
digest verification before any flash writes. Its SHA-256 is
`47961e12177917a6944af443788c496d0497e194971b402c1dc979b0f143102d`.
The v0.1.1 bootloader, partition table and application were then flashed as
separate segments; all three passed the tool's hash verification.
NVS and PHY data were outside the written regions. No eFuses were changed.

The new no_std application is 893,008 bytes. Static DRAM sections total
123,420 bytes, including the 65,536-byte internal heap reservation. The
180-second observation passed: one startup banner, no panic/restart markers,
and successful WiFi sniffer, BLE scan/advertising, display and buzzer
initialization logs. Five periodic status samples at uptimes 31–151 seconds
reported scanning enabled and **5,296 bytes free heap**, unchanged across
samples. This is tight headroom, despite short-run stability. PSRAM is not
initialized or used. No filtered WiFi/BLE result messages were observed during
this interval, so the check does not establish reception effectiveness.

Artifacts, raw logs, backup, exact ELF, source patch and manifest are local at
`target/hardware/m5stickc-20261119/`. The source was a dirty working tree; the
manifest records the base revision and patch hash, rather than representing
this as an official release. Display appearance, audible buzzer behavior,
battery operation and long-run memory stability remain unverified.

### WiFi startup correction

The follow-up firmware starts the station interface before enabling the sniffer
and records channel-hop failures instead of ignoring them. Its 902,096-byte
application was flashed with all three segments hash-verified. In a 100-second
observation, the sample at 91 seconds reported **876 received and parsed WiFi
packets**, zero channel-hop failures, and zero signature matches. Three BLE
result messages were also received. There was one boot and no panic/restart
markers. Free heap remained **3,672 bytes** across all three status samples.

The shared event queue dropped 77 WiFi events (72 already present at the first
sample), so this confirms working reception, not lossless capture. Memory
headroom and burst handling still need improvement. The display can still show
WiFi zero because it counts matches. Follow-up build, flash, source patch,
hashes, ELF and runtime evidence are in `wifi-fix-01/` under the artifact
directory above; the original flash backup and initial observation are retained.

### Observed addresses and signature hits

The M5Stick status screen separates **APs**, **Other WiFi**, and **BLE
devices** from **Signature hits: WiFi / BLE**. Counts are unique observed
addresses since boot, not packet totals or guaranteed physical-device counts.
WiFi beacon/probe-response transmitters count as APs; probe-request/data
transmitters count as other WiFi until a beacon/probe response identifies them
as an AP. Unknown/control frames and invalid WiFi source addresses are excluded.
BLE and WiFi addresses are tracked separately. BLE address type is not retained
by the existing event format, so public/random addresses with identical bytes
are not distinguished. Address rotation can count one physical device more
than once. A multi-BSSID AP can also contribute multiple AP addresses.

Signature hits are a subset of these observations and retain the existing
heuristics and signal threshold; they are not confirmed identifications.
Observed-address counts include processed events regardless of the signature
RSSI threshold. Stop pauses updates without clearing the table; reboot clears
it. Events lost before processing cannot contribute to either count.

The initial allocation-free tracker retained the first 128 transport/address pairs in
897 bytes of static storage. It does not evict entries and inflate totals on
rediscovery. Once another address is seen, `(limit)` appears: all counts then
represent lower bounds, while existing entries can still gain AP/signature
classification. Periodic `Observed addresses` logs expose the same counts and
limit flag without identifiers. Existing serial/GATT match messages and packet
match counters retain their original semantics.

At that stage, PSRAM was physically present but still unused. The pinned
ESP allocator supports internal/external memory capabilities. Enabling PSRAM
requires verifying this board's initialization and keeping restricted radio/
DMA allocations in compatible memory. Report internal free memory separately
from external free memory so a large combined total cannot hide radio pressure.

The counts build passed a 100-second hardware observation: at uptime 91 seconds,
44 AP addresses, 21 other WiFi addresses, and 63 BLE addresses were tracked,
with zero signature hits. All 128 slots were occupied, but no overflow had
been observed at that sample. WiFi received/parsed 770 packets, with 37 queue
drops already present at the first sample and no subsequent increase. There
were no channel-hop errors, panics, or reboots; all three free-heap samples
were 3,672 bytes. The application is 903,920 bytes, all flashed segments passed
hash verification, and evidence is under `device-counts-01/`. Screen rendering
compiled and its rows fit the display bounds; physical appearance has not been
independently observed. Validation also passed 312 core tests, 35 Python tests,
strict core Clippy, formatting, and whitespace checks.


### PSRAM allocation policy

The `m5stickc` feature now enables HAL PSRAM support, with a fixed 2 MiB size
for the Plus2's PICO-V3-02 package. Startup writes and verifies two complementary
address-dependent patterns across the entire mapping before any external
allocation. A mismatch fails startup; it is not treated as usable memory.

A separate `EspHeap` owns PSRAM. The global allocator retains internal RAM,
including existing radio/RTOS allocations, so enabling PSRAM cannot silently
move unreviewed allocations outside internal RAM. The first application user
is a 4,096-entry observation table, initialized directly in external storage
without constructing a large temporary on the internal stack. It retains the
same address deduplication and explicit overflow semantics. This is 32 times
the original capacity. Lookup scans occupied entries only. PSRAM access occurs
in task context; radio callbacks continue to use the internal scan queue.

The M5Stick internal heap remains 64 KiB. A 72 KiB trial failed the linker's
internal-memory boundary and was not flashed. A 68 KiB trial linked but left
only 3,324 bytes of startup stack (versus 6,708 in the prior working build) and
stalled during radio initialization despite passing the PSRAM self-test.
That trial is retained in `psram-01/`; restoring 64 KiB leaves 7,420 bytes of
startup stack. PSRAM does not remove
that boundary or the radio's internal allocation requirements. XIAO retains
its existing internal heap and 128-entry tracker; its PSRAM is not enabled.

`heap_free` in existing status messages remains internal free bytes. The screen
and `Memory stats` logs show internal and external free memory separately.
To require evidence of the PSRAM boot test and external-memory samples:

```sh
python scripts/monitor-firmware.py --port /dev/cu.usbserial-DEVICE \
  --output target/hardware/new-psram-observation --seconds 180 \
  --board m5stickc_plus2 --version 0.1.1 --require-psram
```

This option requires observing startup, not attaching after the startup test.
The initial self-test runs before radio initialization; subsequent packet
reception and growing observation counts exercise PSRAM alongside WiFi/BLE.

The final PSRAM build (`psram-02/`) passed a 180-second hardware observation,
including the full 2 MiB memory self-test. All five samples reported 3,672
internal free bytes and 2,068,472 external free bytes. At uptime 152 seconds,
it tracked 55 APs, 26 other WiFi addresses and 68 BLE addresses (149 total),
with one BLE signature hit and no capacity limit. WiFi received/parsed 1,106
packets; channel-hop errors remained zero. The 42 queue drops were already
present at the first sample and did not increase. There was one observed boot,
no panic/restart markers, and BLE scanning/advertising and display initialized.
This establishes short-run coexistence and PSRAM use, not a long-duration soak.

The 915,152-byte application and all three flash segments passed verification;
NVS and the original full backup were preserved. Both M5Stick and XIAO builds
passed, along with 313 core tests, 36 Python tests, strict core Clippy,
formatting, and the core coverage gate (89.76% lines against 89% required).
The display reports internal RAM and PSRAM separately. Internal heap pressure
has not been solved by adding external memory: the next memory improvements
must reduce internal allocations or explicitly move additional compatible
application data, while retaining sufficient startup stack space.


### Startup stack regression gate

The M5Stick no_std CI build runs `scripts/check-firmware-memory.py` against the
linked ELF and rejects a CPU0 startup-stack reservation below 6 KiB. The floor
is below the previously working 6,708-byte image and above the observed failed
3,324-byte trial. Missing, duplicate or reversed linker boundaries fail closed.
It does not measure task-stack high-water marks or prove worst-case stack use;
those runtime checks remain #153. The regression tests use the actual working
and failed image boundaries rather than mirroring the threshold calculation.
