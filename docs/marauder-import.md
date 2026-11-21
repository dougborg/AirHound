# Marauder passive Wi-Fi capture import

`airhound-marauder-import` is an experimental, **offline source-build tool**. It
imports saved classic PCAP or a binary serial capture into NDJSON using AirHound's
shared identity/signature rules and silent evidence policy. It does not open USB,
flash firmware, send CLI commands, start scans, attach host GPS, or export WiGLE.
It is not included in the published 0.2.0 desktop archives.

```sh
cargo +stable run --manifest-path hostd/Cargo.toml --bin airhound-marauder-import -- \
  capture.pcap --source-id awok-cli-chip > observations.ndjson
cargo +stable run --manifest-path hostd/Cargo.toml --bin airhound-marauder-import -- \
  capture.serial.bin --serial --source-id awok-cli-chip > observations.ndjson
```

Choose a distinct sensor/chip identity for each capture source; USB bridge names
can collide. Input must be an unmodified binary file, not terminal-rendered text.
Keep captures and output private: both contain observed addresses and payloads.
No downloaded capture is automatically trusted as coming from a particular firmware.

## Supported profile and framing

The decoder follows upstream Marauder **v1.21.0**, commit
[`d9197cf063cede379104ca8673c2d952d66575a3`](https://github.com/justcallmekoko/ESP32Marauder/tree/d9197cf063cede379104ca8673c2d952d66575a3):

- [PcapHeader.cpp](https://github.com/justcallmekoko/ESP32Marauder/blob/d9197cf063cede379104ca8673c2d952d66575a3/esp32_marauder/PcapHeader.cpp): little-endian classic PCAP 2.4, DLT105 (bare 802.11), zero time-zone/accuracy fields.
- [configs.h](https://github.com/justcallmekoko/ESP32Marauder/blob/d9197cf063cede379104ca8673c2d952d66575a3/esp32_marauder/configs.h): supported snapshot lengths 1514, 2324, or 4096 bytes.
- [Buffer.cpp](https://github.com/justcallmekoko/ESP32Marauder/blob/d9197cf063cede379104ca8673c2d952d66575a3/esp32_marauder/Buffer.cpp): packet header and payload appended atomically; chunks wrapped in `[BUF/BEGIN]` / `[BUF/CLOSE]` without escaping.
- [WiFiScan.cpp](https://github.com/justcallmekoko/ESP32Marauder/blob/d9197cf063cede379104ca8673c2d952d66575a3/esp32_marauder/WiFiScan.cpp): `beaconSnifferCallback` removes four FCS bytes from management packets in passive AP/probe/raw capture modes. The importer does not heuristically remove another suffix.
- [CommandLine.cpp](https://github.com/justcallmekoko/ESP32Marauder/blob/d9197cf063cede379104ca8673c2d952d66575a3/esp32_marauder/CommandLine.cpp): passive `sniffraw`, `sniffbeacon`, and `sniffprobe` accept `-serial`. `SavePCAP` must be enabled even for serial; an SD card is not inherently required for this path.

Only complete records are accepted. Serial close markers are checked at record
boundaries, so identical bytes inside packet payloads remain packet data. The
first chunk must contain the global header; later chunks contain complete records.
Up to 64 KiB of console bytes are allowed before/between/after chunks. A capture
is bounded to 64 MiB in memory and validated completely before output begins.
Oversized inputs must be split into complete capture sessions, not arbitrary byte
ranges. Concatenated sessions, mixed log/GPX streams, PCAPNG, other linktypes,
big-endian/nanosecond captures, unequal captured/original lengths, corruption and
incomplete chunks fail closed. There is no speculative resynchronization after
corruption. Transport/output errors produce a nonzero exit; an output-write error
can still leave an incomplete output file.

## Evidence and limitations

Every packet retains exact `frame_bytes`, source ID and raw `timestamp_micros32`.
The clock is device boot-relative, wraps after about 71.6 minutes and may reset;
the importer does not infer epoch time or unwrap ambiguous gaps. `rssi_dbm`,
`receive_channel`, `loss_count`, and `gps` are explicitly null. Stock firmware can
silently drop packets when both buffers fill, so a valid import is not proof of
complete capture. A channel advertised in an IE is not receive-channel evidence.

Supported beacon, probe request and probe response identities use the shared
identity-only matcher with silent alert policy. The importer requires a complete
fixed management header, an unfragmented/unprotected layout without DS or optional
HT-control flags, and a fully bounded IE chain containing exactly one SSID of at
most 32 UTF-8 bytes. Incomplete headers/IEs, duplicate/missing/binary SSIDs and
unsupported layouts remain raw-only. Unknown IEs are length-checked rather than
semantically interpreted; this is not a complete 802.11 conformance validator. Full raw bytes are
preserved, but this initial tool does not add new vendor-payload, Remote ID or BLE
decoding. `decoder_profile` identifies the parser contract;
`firmware_version_verified: false` prevents mistaking it for observed firmware
version provenance.

Synthetic fixtures cover marker collisions, mixed console data, timestamp wraps,
invalid headers/lengths, truncation and evidence metadata. **Hardware acceptance
is pending.** The user's original AWOK Dual Touch was observed running v1.10.2;
that is not validated by a v1.21.0 source-profile test. Live serial support,
reconnect/session handling and real capture fixtures remain in
[#181](https://github.com/dougborg/AirHound/issues/181) and
[#175](https://github.com/dougborg/AirHound/issues/175).
