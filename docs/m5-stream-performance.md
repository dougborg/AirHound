# M5 observation-stream throughput

The no_std M5StickC Plus2 stream uses 460800-baud UART, a 20-slot parsed-event
queue and a four-slot serialized-output queue. Both processing and output tasks
yield before acquiring the next record. Yielding after acquisition can retain
large buffers in the suspended task, consuming DRAM otherwise reserved for the
startup stack. Display initialization and consumer tasks are polled before radio
initialization, so the synchronous LCD reset does not block an already-active
scanner at boot.

The internal heap remains 64 KiB and the existing observation table remains in
PSRAM. The queue allocation trades relatively large 512-byte serialized slots for
smaller parsed events, rather than expanding internal-memory use indiscriminately.
Startup-stack reservation is 7,396 bytes, above the strengthened 7,168-byte gate.

UART configuration is applied once; its hardware ownership guards are retained
for firmware lifetime without retaining the driver handle in the main async task.
The ROM diagnostic writer remains synchronous, so this is a measured mitigation,
not an asynchronous UART implementation. The new line rate shortens those stalls.
XIAO and ESP-IDF console rates remain unchanged. Flash/boot-ROM traffic remains
115200; only the no_std M5 application console changes rate. Cargo/just M5 flash
recipes select the correct monitor rate.

## Reproduce

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

The final startup-order trial and its longer-run result are recorded below once
complete. Neither serial validation nor zero queue-drop counters establish
lossless RF reception or BLE notification throughput.
