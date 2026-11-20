# External Wi-Fi observation files, version 1

This is the initial data boundary between the independent MT7610U receiver and
AirHound tooling. The driver is a separate GPL-2.0-only executable; the MIT
consumer does not link or import driver code. Version 1 describes a bounded
observation file, not a process-control protocol or a WiGLE export format.

Each UTF-8 line is one JSON object followed by a newline. Maximum line length
is 16 KiB including the newline; maximum file length is 64 MiB. An empty file,
truncated final line, malformed record or unsupported schema version fails
validation. One file contains one session, with consecutive sequence numbers
starting at zero. Additional fields may be ignored for compatible extensions.

Required record fields:

| Field | Meaning |
| --- | --- |
| `schema_version` | Integer `1` |
| `type` | `beacon` |
| `source` | `mt7610u-lab` for this initial producer |
| `session_id` | Nonempty opaque session identifier, at most 128 UTF-8 bytes, without control characters; not a device identifier |
| `sequence` | Consecutive unsigned integer starting at zero |
| `observed_at_unix_ms` | Host wall-clock timestamp when the observation was decoded; not hardware TSF |
| `elapsed_ms` | Monotonic milliseconds since the capture loop began |
| `bssid` | Six colon-separated uppercase hexadecimal bytes, a nonzero unicast BSSID |
| `ssid_bytes` | `null` when absent; array of up to 32 bytes otherwise. Empty means hidden/empty; non-UTF-8 bytes are preserved |
| `receive_channel` | Configured HT20 channel: 1–13; 36–64 or 100–144 in steps of 4; 149–165 in steps of 4 |
| `advertised_channel` | Beacon's advertised primary channel, or `null` when unavailable |
| `channel_epoch` | Zero initially; increments after each completed retune |
| `dwell_elapsed_ms` | Time since the current channel became available for capture |
| `rssi_dbm` | `null` for this prototype; no independently validated signal value |
| `estimated_rssi_dbm` | Signed byte or `null`; EEPROM-derived estimate |
| `rssi_estimate_basis` | `mt76_eeprom_formula` |
| `rssi_validated` | `false` |
| `raw_rssi` | Four descriptor bytes; only chain zero is used by MT7610U |
| `full_calibration` | `false` |

Configured channel and advertised channel must remain separate. Strong nearby
APs can appear off-channel, and the hardware cause of observed far-channel
sightings remains unproven. Estimates are not silently promoted into calibrated
RSSI. The timestamps do not provide location; replay must not attach the current
Mac's location to historical observations.

Validate a completed capture from the `hostd` directory:

```sh
cargo run --locked --bin airhound-capture-check -- /path/to/beacons.ndjson
```

The checker emits aggregate counts, not captured network identifiers. It does
not open USB, start another executable, run surveillance matching, or write
WiGLE CSV. Existing pre-versioned prototype files are intentionally rejected.

Live integration still needs a host-only observation model preserving nullable
signal values, an explicit estimated-signal export policy, and a supervised
child-process lifecycle with pause/resume and verified shutdown. Those semantics
must be established before this file format becomes a live scanner backend.
