# M5 observation-stream throughput

## Historical hardware checkpoint — 2026-11-23

At this checkpoint, the installed `96b19ae` / `0.2.0` image (`d7e383a0`)
completed ten minutes without a printed panic or assertion and retained late
Wi-Fi/BLE continuity. Its strict acceptance failed for missing health reports,
reported output loss and varying internal memory; see the
[hardware ledger](attached-hardware-validation.md#current-m5-uart-correction--2026-11-23-utc).
Its four sampled diagnostic `log_drops` were included in the 30 `output_drops`,
not additional losses. The independently reviewed `3fbc1a0` follow-up in
[#321](https://github.com/dougborg/AirHound/pull/321) yielded between remaining
health-report enqueues. Its target build and packaging had passed at that point;
artifact audit and hardware acceptance were still pending, and it was not yet
installed. These statements retain the original checkpoint, not current delivery
status or acceptance of the later architecture below.

## Current candidate architecture

The current no_std M5StickC Plus2 output architecture uses 460800-baud UART,
a 20-slot parsed-event queue and a fixed 64-slot PSRAM serialized-output pool,
including eight slots reserved for health/command admission. A separate four-slot
internal-RAM channel stages runtime logger messages from arbitrary contexts;
a task transfers them into the pool. The single writer retains only an exclusive
pool lease and small scratch buffer across cooperative yields, not a copied
512-byte record. Holding a full record in a suspended task can consume DRAM
otherwise reserved for the startup stack. Explicit one-shot signals gate radio
initialization on consumer readiness and
the completed splash-to-status redraw. Display failure releases the barrier in
degraded mode. WiFi capture remains disabled until the BLE host scan-start
attempt completes; its channel-hopping task starts with capture. The fixed
two-second splash is UI time, not a guessed radio-settling delay. Normal scan
intervals and queue/drop accounting are unchanged.

The internal heap remains 64 KiB and the existing observation table remains in
PSRAM. The 64 × 512-byte output payload storage is also allocated in PSRAM at
startup; the bounded logger staging channel remains in internal RAM.
The earlier startup-order image reserved 7,396 bytes for the stack. Every new
M5 image must independently pass the unchanged 7,168-byte linked-reservation
gate; this is not a runtime high-water measurement.

UART configuration is applied once at 460800 baud. XIAO and ESP-IDF console
rates remain unchanged. Flash/boot-ROM traffic remains 115200.

## Cooperative M5 HCI receive candidate

The M5-only HCI transport yields once before each packet read. The underlying
connector can return queued packets immediately; the BLE host runner otherwise
may consume an entire backlog in one executor poll. This can fill the shared
20-slot parsed-event queue before its filter consumer runs and delay UART work.
The new boundary gives other ready tasks an opportunity between packets without
adding a timer, packet copy, scan-duty change, duplicate filtering, or UART-budget
change. Cancellation at that boundary has not consumed the next packet. Command
writes and XIAO transport behavior are unchanged.

This is a source scheduling correction, not proof of the cause of an observed
23,006-microsecond poll gap or hardware acceptance. One HCI event can contain
multiple reports delivered by a synchronous callback; a single large batch can
still exceed queue capacity. WiFi callbacks remain nonblocking, and sustained
input above processing capacity can still drop events. Scan admission losses are
separate from serialized-output loss and unknown RF loss. Existing thresholds
and historical failed receipts remain unchanged. Portable tests cover the actual
transport wrapper and packet decoding with a bounded queue, including the
remaining single-packet burst limit. Exact target memory and live behavior must
be checked independently.

## Bounded M5 UART output candidate (#320)

An observed controller assertion in `lld_evt.c` stopped firmware while the host
reader remained open. Its cause remains unproven. Pinned `esp-println` holds an
interrupt-masking lock around a complete synchronous UART log line; BLE
interrupts are lower priority than that mask. Actual masked duration was not
measured. The output candidate removes this known scheduling hazard without
changing scan mode, intervals, or duplicate filtering. It is not yet hardware
acceptance or a proven fix for [#320](https://github.com/dougborg/AirHound/issues/320).

After startup, all ordinary M5 diagnostics and JSON records reach the PSRAM pool
and a single UART owner. Observation/diagnostic producers use bounded nonblocking
admission; commands may await a reserved slot. Diagnostics flatten CR/LF and
escape opening braces: the legacy host looks for JSON after a log prefix, so
the prefix alone cannot prevent diagnostic text from becoming an observation.
Diagnostics are never forwarded to BLE subscribers. Eight reserved slots improve
structured-health/command admission without guaranteeing lossless delivery.

The UART task checks FIFO readiness and copies at most 16 bytes in one short
ownership critical section. It yields immediately on blockage and after at most
four steps (64 bytes), with interrupts enabled outside the brief FIFO copy.
The cooperative writer uses a 40 ms no-acceptance watchdog and
an immutable length-aware total allowance capped at 200 ms; see the exact
[service-budget policy](m5-psram-output-proposal.md#prospective-cooperative-uart-service-budgets).
Positive validated FIFO acceptance renews only the watchdog. The 94/310/512-byte
allowances are 62.040 / 126.728 / 191.112 ms; actual completion does not wait for
them. These are poll-time limits, not preemption or measured wire latency.
A completed final-step overrun is reported separately and never retried as an
incomplete frame. Bounded elapsed/overrun/last-timeout diagnostics require no new
payload buffer. Successful delimiter recovery does not erase a timeout loss.

> **Historical dual-budget planning checkpoint:** The initial proposal retained
> a separate <20 ms intra-line poll-gap responsiveness gate, the zero-output-loss
> criterion and the existing memory gate. Portable tests existed, while the final
> target build, linked-memory audit and physical acceptance were still pending.
> These statements record the plan at that time: the old poll-gap gate is not a
> current service-budget requirement, and the pending-work list is not current
> delivery status. The prior 600-second display/resynchronization attempt had
> continued both radios and recovered after four typed timeout losses; its strict
> verdict remains failed. That observation did not establish which task or FIFO
> condition caused each timeout. No historical benchmark below measures the new
> budget policy.

Boot diagnostics are synchronous only before the runtime ownership handoff and
radio readiness barrier. The panic pre-backtrace hook permanently takes the same
ownership lock before the existing newline/backtrace output. An interrupted
ordinary line is abandoned and never resumed. Controller-blob assertion text
written before that hook is outside normal framing guarantees and remains fatal
diagnostic evidence. XIAO keeps its existing console implementation.

Periodic M5 health publication yields between diagnostic and structured records.
Serialization remains synchronous in helpers, so no 512-byte buffer survives
these yields. The historical four-slot regression reproduced health-record
rejection from a same-poll burst; current pool admission and reserved slots are
separate bounds. Competing producers can still exhaust ordinary admission, and
those rejections remain counted. Sampled pool diagnostics distinguish occupancy,
queue residence, writer poll gaps, record classes and loss reasons.

An ordinary payload timeout retains the exclusive draining lease for one LF-only
resynchronization attempt under a separate fixed 20 ms budget. Successful LF
recovery releases the damaged record and permits ordered output to continue;
the original record still counts once as lost. Its payload and BLE admission
are never retried. A hard UART error, invalid progress, failed LF recovery,
or cancellation closes normal UART admission until reboot. Panic takes UART
ownership permanently before backtrace output. The closed pool is never reopened. Ordered bounded BLE fallback can continue for JSON,
with separate admission/loss accounting; this does not prove BLE delivery.

The periodic `Serial output stats` diagnostic distinguishes logger drops,
format failures and UART failures. `Output pool` reports typed pool losses and
`UART service` reports elapsed timing, completed overruns and the last timeout's
length/accepted prefix. These reports are themselves bounded and fallible. Legacy
`output_drops`/`serialize_errors` retain applicable loss evidence, and a permanently
closed UART cannot promise a final serial counter report. No error path logs
recursively.

## What the loss counters mean

`scan_drops` counts parsed observations that could not enter the input queue
because it was full. The radio received these observations, but detection
processing and streaming never saw them. This is an event count, not a count of
unique devices: repeated advertisements from one device can each be dropped.
A subsequent advertisement may still identify the device; a brief observation
may be lost entirely.

`output_drops` counts records rejected by the serialized-output queue and, in the
M5 bounded-output candidate, records rejected after UART failure or timeout.
Runtime diagnostic queue drops are included; this is no longer exclusively an
observation count. Diagnostic formatting failures also increment
`serialize_errors`.
`ble_output_drops` counts records rejected by the BLE notification queue when a
subscriber is connected. These counters identify different stages and should not
be added to estimate the number of unique devices missed. None counts packets
that the radio never received, including traffic on a different WiFi channel.
Counters are cumulative since boot; compare consecutive samples to locate loss.

## Reproduce

The command below documents the historical 0.1.1 experiment procedure, not
acceptance of the current candidate. New candidate testing uses a separately
reviewed image-bound observer with current timing and loss diagnostics.

Use a compatible data cable, release the device from any VM USB forwarding,
close other serial readers and keep the radio environment as consistent as
possible. Record the exact source/binary hash before flashing. Capture from a
fresh boot with the script's explicit reset option:

```sh
python scripts/monitor-firmware.py \
  --port /dev/cu.usbserial-59010084991 --baud 460800 \
  --reset --require-psram --version 0.1.1 --seconds 300 \
  --output target/hardware/new-private-run
```

The script requires pyserial. Use 115200 for the old baseline. Each output
location must be new; raw observations are private. Compare numeric summary
fields: observation counts, startup count, heap range, WiFi reception and hopping
errors, and all stream-loss samples. A passing stability check alone does not
establish zero loss. Dropping or reducing reception must not be mistaken for an
improvement; uncontrolled RF traffic makes these comparisons observational.

## 2026-11-20 experiments

All compared runs used the same M5, WiFi channel plan/dwell, continuous BLE scan,
USB bridge and signature policy. No BLE subscriber was connected. Two-minute
runs produced periodic snapshots around uptime 32, 62 and 92 seconds; counters
below are the last snapshot, not a claim about the unreported final interval.

| Candidate | WiFi / BLE records in capture | Scan drops at snapshots | Last output drops |
| --- | ---: | --- | ---: |
| Original 115200-baud stream | 1788 / 420 | 94, 154, 178 | 7 |
| Correctly scoped task yields | 1843 / 220 | 70, 80, 100 | 0 |
| 460800 baud; 8 scan / 7 output slots | 1040 / 153 | 43, 43, 48 | 0 |
| 460800 baud; 20 scan / 4 output slots | 967 / 171 | 35, 35, 35 | 0 |

WiFi traffic declined during later tests, so the serial-speed and queue results
are not controlled throughput benchmarks. The high-baud trial also failed the
startup-log assertion: boot-ROM bytes at the previous baud ran into the first
PSRAM log line. An explicit newline at the new baud resolved that framing issue
in the following trial. No serialization failures were observed in these runs.

An earlier, rejected yielding implementation reduced startup-stack reservation
to 6,556 bytes and hit the hardware stack guard during boot. The active boot beep
then stayed on until the known-working image was restored. This candidate was
not retained; the static gate is now 7 KiB. Other candidates below that gate were
built but not flashed. Static reservation cannot replace runtime validation.

The final startup-order trial captured 2,454 WiFi and 743 BLE records over 300
seconds. All nine snapshots (uptime 32 through 272 seconds) reported 55 scan
drops, all attributed to WiFi, with zero output drops or serialization errors.
Internal free heap stayed at 3,672 bytes; there were no channel-hop errors or
reboots, and the PSRAM startup check passed. Thus no additional input-queue loss
was reported across that 240-second interval, but 55 events were lost before the
first snapshot. Moving display startup alone did not eliminate early loss.
All 3,215 complete protocol records passed the device-message schema.

The diagnostic follow-up logs first/last scan-drop times in milliseconds since
boot (wrapping after about 49 days), without logging from radio callbacks. These
aggregate timestamps help distinguish initialization stalls from later bursts.
They do not change the wire protocol's cumulative loss counters.

Neither serial validation nor zero queue-drop counters establish lossless RF
reception or BLE notification throughput.

A subsequent 90-second diagnostic run located 59 losses between 2,016 and
2,354 milliseconds after boot. Yielding immediately after enabling promiscuous
capture was then tested for 90 seconds: 698 WiFi and 203 BLE records were
captured, with 38 losses between 2,076 and 2,335 milliseconds, zero output losses,
and stable free heap. RF traffic was uncontrolled; the lower count does not
establish a causal improvement. The early startup loss remains unresolved.
The explicit yield is retained to let consumers run before further BLE/GATT
setup; it is not claimed to eliminate the initialization stall.

## Startup readiness validation

The evidence-build baseline recorded 64 scan queue drops between 1,945 and
2,354 ms after boot, then no further increase through 122 s. The splash screen
previously cleared and drew its first status page after scanning had started.
The readiness barrier moves that synchronous drawing ahead of radio startup.
Capture-consumer and radio-ready timestamps make the sequence observable.
Repeat fresh-boot captures to verify the change; a gate prevents intentional
early reception and does not prove RF losslessness.
