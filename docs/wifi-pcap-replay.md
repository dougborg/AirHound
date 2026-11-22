# Offline WiFi PCAP replay

Replay a saved classic PCAP with radiotap headers through the same WiFi parser,
signature matcher, NDJSON output and WiGLE CSV writer used by monitor capture:

```sh
cargo +stable run --manifest-path hostd/Cargo.toml --locked -- \
  --wifi-pcap capture.pcap --output replay.csv > replay.ndjson
```

This standalone option selects only the saved WiFi source. It does not initialize
BLE, CoreWLAN, CoreLocation, a monitor interface or a receiver helper. It conflicts
with other source selectors and `--gpsd`; use an explicit session file to combine
sources. Output-file overwrite protection still applies. Replay processes records
as quickly as bounded downstream queues allow, without reproducing original timing.
EOF ends the source cleanly; the process exits when all selected sources finish.
Pause/resume preserves position; shutdown cancels a paused or backpressured source.

## Sessions

```json
{
  "version": 1,
  "receivers": [
    {"id":"recorded-wifi", "source":{"kind":"wifi_pcap", "path":"capture.pcap"}}
  ]
}
```

Every observation retains `session_source.receiver_id` and ingress sequence, so
concurrent recordings remain distinguishable. Replay-only sessions do not start
CoreLocation. Mixed live/replay sessions may run explicitly configured live sources;
replayed records never use the host's current location, even in such a session.

## Metadata and exports

NDJSON observations and paired evidence add `replay: true` and `capture` fields:

```json
{"replay":true,"capture":{"captured_at_us":1700000000123456,"record_sequence":1,"captured_len":42,"original_len":42,"timestamp_resolution":"microseconds"}}
```

`capture.captured_at_us` is the PCAP timestamp, not replay time. Nanosecond captures
are truncated to microseconds and label that conversion. Existing `ts` remains
host-session elapsed time; do not confuse the two clocks. `record_sequence` counts
PCAP records, including records later rejected; a packet can yield both WiFi and
Remote ID events. WiGLE `FirstSeen` uses the captured timestamp in UTC, with its
format's second precision, and location cells remain empty. Receive frequency and
band come from radiotap; overlapping channel numbers are not used to guess a band.
See the [frequency contract](wifi-observation-contract.md).

`wifi_replay_status` reports final state (`finished`, `stopped`, or `failed`), input
record count, rejected records and emitted events. Radio loss is unknown. The
replay queues apply backpressure rather than deliberately dropping observations.
Rejected records include incomplete captures (`captured_len < original_len`), bad
FCS, missing/unsupported receive metadata and frames the parser cannot interpret.

## Alert policy

WiFi PCAP replay uses the shared WiFi matcher and the host session's alert policy;
replay is not an automatic `silent` override. Use `--alert-policy silent` when only evidence
inspection is wanted, or set `"alert_policy":"silent"` in the session file. This is
separate from external/import paths that deliberately force silent evidence.
Every replay observation and paired detection is marked `replay: true`; consumers
must keep recorded results distinct from live notifications. This CLI writes
records and does not itself trigger a desktop notification or a hardware buzzer.

## Limits and acceptance

The opened file handle is checked for regular-file type; Unix opens are nonblocking
so a FIFO replacement cannot wait for a writer. Only PCAP 2.4 and exact link
type 127 (radiotap) are accepted.
Both byte orders and microsecond/nanosecond timestamp magic values are supported.
PCAPNG and other link types are rejected; convert explicitly with a separate tool
if needed. Each captured record is limited to 65,536 bytes before allocation;
invalid lengths, timestamps, truncated file headers or truncated record bodies
fail replay with a nonzero exit. Global snaplen must be positive and at most
16 MiB, but never permits exceeding the per-record allocation bound.

Synthetic tests exercise overlapping 2.4/6 GHz channel 1, timestamp/source
preservation, CSV frequency/location behavior, EOF, filtering, bounded parsing,
pause and cancellation. These do not establish WiGLE service import acceptance,
live adapter support, tuning, or RF capture correctness. In particular, replaying
an mt76-generated file does not activate an AXML/A9000 driver in AirHound (#235).
