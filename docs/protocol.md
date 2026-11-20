# Observation and command protocol

[Project home](../README.md) · [Documentation index](README.md) · [Schemas](../schemas/README.md)

AirHound uses newline-delimited JSON (NDJSON) for all communication. The format is transport-agnostic — the same messages work over BLE GATT, serial, TCP, or any stream transport. The contracts below describe data formats; available transports depend on the runtime. A TCP or BLE transport mentioned in the format does not imply a client implementation.

### Device Messages (sensor -> host)

**Matched WiFi observation:**
```json
{"type":"wifi","mac":"B4:1E:52:00:00:01","ssid":"Flock-A1B2C3","rssi":-65,"ch":6,"frame":"beacon","match":[{"type":"mac_oui","detail":"Flock Safety"},{"type":"ssid_pattern","detail":"Flock Safety camera WiFi"}],"ts":12345}
```

**Matched BLE observation:**
```json
{"type":"ble","mac":"58:8E:81:00:00:02","name":"FS Ext Battery","rssi":-72,"mfr":2504,"match":[{"type":"ble_name","detail":"FS Ext Battery"},{"type":"ble_mfr","detail":"Known manufacturer ID"}],"ts":12346}
```

**Status report:**
```json
{"type":"status","scanning":true,"uptime":3600,"heap_free":45000,"ble_clients":1,"board":"xiao_esp32s3","version":"0.1.0"}
```

Ordinary firmware observations have the same fields with `"match": []` and no
`rule`. Check match metadata before alerting. Serial output also contains log
prefixes and diagnostics; it is not a clean bidirectional NDJSON port. Firmware
commands are received over BLE GATT, whereas hostd receives its own commands on
stdin. [Stream counters and framing](../docs/firmware-streaming.md) explain bounded,
best-effort delivery and BLE reassembly.

### Host Commands (host -> sensor)

```json
{"cmd":"start"}
{"cmd":"stop"}
{"cmd":"status"}
{"cmd":"set_rssi","min_rssi":-80}
{"cmd":"set_buzzer","enabled":false}
```

### BLE GATT Service (ESP32 firmware)

| Attribute | UUID | Properties |
|-----------|------|-----------|
| Service | `4a690001-1c4a-4e3c-b5d8-f47b2e1c0a9d` | -- |
| TX (results) | `4a690002-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Notify |
| RX (commands) | `4a690003-1c4a-4e3c-b5d8-f47b2e1c0a9d` | Write |

## Command results and runtime support

Each command dequeued by a runtime emits a `command_result` after applying its
supported state changes. A `status` request also emits the existing status
message. Malformed or unknown input that reaches the command queue returns
`invalid_command` with `cmd: "unknown"`.

```json
{"type":"command_result","cmd":"start","ok":true}
{"type":"command_result","cmd":"export_wigle","ok":false,"error":"unsupported"}
```

| Command | no_std firmware | ESP-IDF firmware | hostd |
|---|---|---|---|
| start / stop / status / set_rssi | Supported | Supported | Supported |
| set_buzzer | Supported on configured buzzer boards | M5StickC only | Unsupported |
| add_watchlist / remove_watchlist | Supported, volatile | Supported, volatile | Unsupported |
| set_mode / proximity commands / enable_category / export_wigle | Unsupported | Unsupported | Unsupported |

Firmware command ingestion uses BLE GATT; hostd accepts NDJSON on stdin.
Stopping pauses detection/output; it does not promise that the radio hardware
is powered off. hostd already writes WiGLE CSV continuously, but does not
implement an on-demand export command.

Error codes are `invalid_command`, `invalid_argument`, `unsupported`,
`duplicate`, `capacity`, and `not_found`. Watchlist changes update the list and
lookup index together, reject duplicate IDs/targets, and roll back if the index
cannot accept an entry. RSSI must be between -128 and 0 dBm.

Responses have no request IDs; clients should send one command at a time and
wait for its result. BLE ingress/output queues and notifications remain bounded
and best-effort: an overlong line, full ingress queue, or disconnected client
can still lose a request/response. A missing response is not confirmation of
success. These additive messages extend the v1 schema; consumers should ignore
unknown message types when using newer firmware.
