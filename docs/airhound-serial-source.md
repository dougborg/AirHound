# AirHound firmware as a desktop receiver

The `airhound_serial` source records the existing firmware's WiFi and BLE
observation stream into the same durable session as other receivers. Select a
known AirHound device explicitly: a WCH/CP210x USB bridge alone does not identify
its attached firmware. Opening a UART may reset the board, even without a
bootloader handshake. Passive discovery does not open generic serial ports.

## Configure a receiver

Use the device's actual port and application baud rate. The no_std M5StickC Plus2
build uses 460800; other firmware commonly uses 115200. For example:

```json
{
  "version": 1,
  "alert_policy": "silent",
  "receivers": [
    {
      "id": "m5",
      "source": {
        "kind": "airhound_serial",
        "port": "/dev/cu.REPLACE_WITH_VERIFIED_PORT",
        "baud": 460800
      }
    }
  ]
}
```

Import the plan in the macOS app, or pass it to `airhound-hostd client start
--session-config PLAN --request-id UNIQUE_ID` with the configured service socket.
The same source works in standalone `--session-config` capture. Validation checks
configuration and duplicate serial resources; it does not open the board or
establish RF readiness. A Sniffle source and an AirHound source cannot claim the
same port, including macOS `cu`/`tty` aliases.

The source uses 8N1 without hardware flow control. It sends no commands and does
not manipulate DTR/RTS. The OS or USB bridge can still change those lines on open.
Close other serial monitors first. Port ownership stays exclusive until finish.

## What the recording means

- `wifi_observation` and `ble_observation` contain host evaluation using the
  current signature rules. `producer_record` retains the original firmware
  record, including its own match hints, separately.
- Producer `detection` events remain `airhound_device_detection` with an explicit
  unassociated-evidence label. An address and uptime pair is not a unique event
  identifier, so the host does not guess which observation they annotate.
- `ts` remains device uptime milliseconds, **not UTC or host capture time**.
  Connection and boot generations delimit reconnects and observed uptime resets.
  These generations are host boundaries, not hardware boot IDs. Status `uptime`
  is seconds. Original producer records are not changed.
- `airhound_device_status` retains the reported board, version, uptime and memory
  values. Active receiver identity is populated only after that status arrives;
  disconnect clears the current identity. WiFi/BLE health becomes running
  independently as records from each medium arrive.
- Firmware summaries do not contain full BLE advertising payloads/address types
  or WiFi security/frequency data. Missing fields remain unknown. Serial reception
  does not establish extended advertising or Coded PHY support. This initial path
  does not export these observations to WiGLE or attach host GPS to them.

The reader bounds each line to 4096 bytes, strips diagnostic/ANSI framing and
recovers at the next newline after malformed or oversized input. It reports
`host_dropped_records`, malformed/oversized counts, paused discards and reconnect
attempts separately from the device's `stream_stats`. Device counters cover their
reported queues, not all RF loss. Both output and device counters remain
best-effort; zero counters do not prove lossless scanning.

## Pause, finish and disconnection

Pause **pauses host observation ingestion**. The source keeps its port open and
drains/discards observations, avoiding an open/close reset cycle on resume. Health,
producer status and independently reported producer detection evidence remain
available. Already received records keep their original generation; paused
observations are not replayed as new sightings on resume.

Finish closes the serial transport. Neither action stops the device's autonomous
radios or changes its buzzer. Existing firmware commands are accepted through BLE
GATT, not the diagnostic UART. See the [command protocol](protocol.md).

After a link/read failure the source reports unavailable and retries at two-second
intervals, at most ten consecutive unsuccessful attempts. The configured endpoint
must still identify the intended device; a newly named port requires a new plan.
No recognized record for 60 seconds is a transport timeout. The scanner has a
five-second shutdown budget. Physical unplug/replug acceptance is separate from
the synthetic EOF and framing tests.

See [firmware streaming](firmware-streaming.md) for producer limitations and the
[attached hardware matrix](attached-hardware-validation.md) for bounded live evidence.
