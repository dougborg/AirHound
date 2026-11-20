# Desktop BLE Remote ID

The desktop receiver decodes normalized service-data records for the full
Bluetooth-base service UUID `0000fffa-0000-1000-8000-00805f9b34fb`. Its payload
starts with Remote ID application code `0x0D`, a message counter, then a message
or pack. No generic signature hit, hardware MAC, or reconstructed AD packet is
required. BLE enable/RSSI policy still applies before decoding.

Output uses the separate `ble_remote_id` record described by
[desktop-remote-id.v1.schema.json](../schemas/desktop-remote-id.v1.schema.json).
An opaque backend peripheral ID is retained, and an observed hardware address is
optional. The record identifies its data as an OS property snapshot, possibly
cached, with unknown radio-packet completeness. Decoded identifiers and position
fields are broadcast claims, not authenticated identity or position. This output
does not itself trigger a threat alert.

Each available nontruncated service-data record is independently decoded.
Locally truncated records, wrong UUID/application code, malformed messages and
unrecognized payloads do not produce a decoded sighting. Repeated valid records
remain repeated observations: a counter can wrap or repeat, and a cached update
cannot establish a new over-the-air reception. No fragment reassembly or cross-
snapshot field corroboration is performed here.

## Software validation and hardware limits

The public long-range sample used by the core parser is also exercised through
the desktop decoder. Its source is the Apache-2.0
[Open Drone ID sample captures](https://github.com/opendroneid/wireshark-dissector)
(`odid_bt5_lr_sample.pcapng`). The test fixture removes the AD header and UUID,
leaving the actual service payload; it is not a local wireless capture.

| Receive path | Software fixture decoding | Live legacy reception | Live extended / PHY reception |
| --- | --- | --- | --- |
| Linux BlueZ/btleplug | Backend tests are required before completion | Not established by fixtures | Not validated |
| macOS CoreBluetooth/btleplug | Backend tests are required before completion | Not established by fixtures | Not validated; system API visibility applies |
| M5 no_std firmware | Separate implementation and 62-byte storage limit | Existing firmware path | This desktop work adds no support |

Adapter capability flags are diagnostics, not validation results. An OS may
aggregate, cache or omit advertised fields; absence of a sighting is not proof
that the transmitter was absent. Support for extended advertisements does not
establish support for LE Coded PHY.

## Controlled acceptance procedure

Before testing, record source revision, receiver/controller/firmware, OS/kernel/
BlueZ or macOS version, adapter selection, scan configuration, transmitter model/
firmware, advertising interval and PHY. Use a known transmitter and retain raw
captures locally; publish aggregate counts and configuration.

1. Establish a legacy 1M baseline, including a known valid identifier and changing
   counter. Confirm the expected UUID and available payload arrive through the
   production conversion, decoding and output path with correct provenance.
2. Test supported extended 1M, 2M and Coded combinations separately. Document which
   layer reassembles and what completion metadata reaches the application. Unknown
   report completeness remains unknown even when a message decodes.
3. For each run, count expected transmitter messages where observable, delivered
   updates, valid decoded messages, malformed/truncated records, queue losses and
   process restarts. Record OS duplicate-filter/caching limits on comparisons.
4. Repeat with restart, concurrent WiFi, other BLE connections and queue pressure.
   Keep valid-record preservation and bounded memory explicit acceptance gates;
   define the tolerated RF reception rate from the controlled baseline before the
   load test, rather than retrofitting a threshold to results.
5. Publish per-configuration supported/unsupported/unknown results. Do not promote
   a parser fixture, controller feature bit or one receiver's results into a claim
   about every platform or adapter.

Track acceptance in [#172](https://github.com/dougborg/AirHound/issues/172), with
[#171](https://github.com/dougborg/AirHound/issues/171) for receiver diagnostics and
[#132](https://github.com/dougborg/AirHound/issues/132) for cross-platform reception.
