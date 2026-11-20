# Device signature research — 2026-11-20

AirHound can grow into a wireless device inventory and protocol classifier as
well as a detector for selected signatures. The strongest next additions come
from structured protocol fields and labeled captures, not larger lists of vendor
prefixes. The [research pack](../signatures/README.md) is an initial reviewable
artifact; it does not change runtime detection or alert behavior.

## Opportunity matrix

“BLE path” below means an ESP32 scanner or desktop BLE backend **if the required
fields are actually advertised and delivered**. It is a transport opportunity,
not a claim that a decoder is implemented. A discoverable GATT service need not
appear in advertising packets.

| Class / application | Observable fingerprint and evidential limit | AirHound path / next work | Primary or implementation sources / reuse status |
| --- | --- | --- | --- |
| Meta / Snap glasses | Company ID plus brand-like name; shared IDs also occur on non-glasses. No recording-state or wearer inference. | Offline candidate rules; BLE path. Need labeled paired/unpaired captures and same-vendor controls. | [Nearby Glasses](https://github.com/yjeanrenaud/yj_nearbyglasses) (AGPL-3.0 research lead); [Bluetooth SIG company registry](https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/company_identifiers/company_identifiers.yaml) (factual assignments). No code copied. |
| Other smart glasses | Full HeyCyan custom service UUID plus name; SDK-reported UUID alone does not prove it is passively advertised. | Offline candidate rule; verify full 128-bit UUID and advertising availability. | [HeyCyan SDK](https://github.com/ebowwa/HeyCyanSmartGlassesSDK#ble-service--characteristic-uuids-found-in-binary) (license not verified; no SDK code/binaries copied). |
| Body-camera/vendor equipment research | TASER company `034D` plus `FC81`, `FE6B` or `FE6C` assigned service. Indicates an Axon/TASER ecosystem candidate, not a camera model, recording, or law-enforcement ownership. | Offline candidate rule; BLE path. WiFi vendor matching needs public/universal address semantics. | [AXEOFF](https://github.com/colonelpanichacks/axeoff) (MIT); [SIG member services](https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/uuids/member_uuids.yaml). |
| Item trackers | Find My manufacturer framing; Google Find Hub service-data frame; Tile service assignment. Ecosystem identification is separate from unwanted-following analysis. | Tile candidate included. Find Hub needs typed service-data matching, exact framing and backend fixtures; temporal analysis is separate. | [AirGuard](https://github.com/seemoo-lab/AirGuard) (Apache-2.0); [Google Find Hub specification](https://developers.google.com/nearby/fast-pair/specifications/extensions/fmdn) (protocol reference). |
| Asset beacons / indoor inventory | Eddystone frame type and length, iBeacon manufacturer framing. Shared layouts identify a protocol, not a physical asset or a threat. | BLE path; add AD-context-aware signatures and optional identifier decoding with retention controls. | [Eddystone](https://github.com/google/eddystone) (Apache-2.0); [Theengs devices](https://decoder.theengs.io/devices/devices.html) (see license note below). |
| Environmental / building sensors | BTHome v2 service data `FCD2` plus version/format; structured temperature, humidity, CO₂ and contact payloads. Encryption can prevent measurement decoding. | BLE path; typed service data plus format parsers. Ordinary inventory/measurements, no default threat alert. | [BTHome format](https://bthome.io/format/) (primary protocol); [Theengs Decoder](https://github.com/theengs/decoder) (GPL-3.0 license file). |
| Wearables / health devices | Standard service IDs express capabilities; model-specific framing may be more precise. Heart-rate service alone does not identify a person or exact device. | BLE path; capability classification. Avoid inferring private health facts from a name/vendor clue. | [SIG assigned numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html); [Home Assistant Bluetooth](https://www.home-assistant.io/integrations/bluetooth/) for integration discovery approaches; review each parser's license separately. |
| Vehicles / fleet accessories | Model-specific BLE service/manufacturer payloads for supported vehicle tags or sensors; some integrations require a connection. | BLE candidates can use existing radios. Do not equate a vehicle accessory with vehicle ownership or activity. | [Theengs supported devices](https://decoder.theengs.io/devices/devices.html) includes vehicle-related models; GPL-3.0 implementation reference, not a bulk-import license for MIT defaults. |
| TPMS / weather / utility meters | Sub-GHz modulation and packet structure, often with checksums and model-specific fields. BLE/WiFi scanning cannot receive these bands. | Future external receiver + decoded-event ingest; SDR/appropriate radio needed. | [rtl_433](https://github.com/merbanan/rtl_433) (GPL-2.0 license file). Prefer a separately installed decoder and explicit adapter rather than copying decoder code into the MIT core. |
| LoRa mesh nodes | Complete Meshtastic client BLE UUID, if advertised. Does not expose LoRa traffic, mesh activity or network identity. | Offline interface candidate included. LoRa reception requires separate hardware/protocol support. | [Meshtastic client API](https://meshtastic.org/docs/development/device/client-api/#bluetooth-meshbluetoothservice) (primary protocol reference; no implementation copied). |
| Network cameras / access points | Vendor OUI, model-specific SSID/IE or protocol evidence. Chipset vendors and generic `camera` names are weak. | Existing WiFi paths; need labeled public captures and frame-context matching (#39), not more broad default alerts. | Existing Flock research plus manufacturer manuals/IEEE assignments; [FlockSquawk](https://github.com/f1yaw4y/FlockSquawk) is a lead, not independent proof of every inherited entry. |

## What the source audit changes

The current Bluetooth SIG registries were retrieved on the review date; hashes
are in [the provenance manifest](../signatures/research-provenance.json).
`01AB`, `058E`, `0D53` and `03C2` are vendor assignments, not immutable hardware
identities. Manufacturer-specific AD is optional, and its contents are not
authenticated. The assigned service `FD5F` belongs to Meta Platforms Technologies,
but the registry does not establish exclusivity to Ray-Ban glasses. We did not
promote it to a product detector.

Nearby Glasses and [ESP-GlassHole](https://github.com/surveillancewatch/ESP-GlassHole)
are useful discovery leads. Their broad company matching and sample
`META_RB_GLASS` text do not establish a validated model fingerprint. That token
is excluded from the candidate pack pending real labeled captures. The HeyCyan
SDK publishes a full custom UUID, which is retained as a research candidate with
a name guard. It must not be shortened to `FFF0`.

[AXEOFF](https://github.com/colonelpanichacks/axeoff) provides a focused equipment
research lead. We independently checked the three service assignments and the
company assignment against the SIG registries. We did not import broad claims
that every such advertisement is a body camera. `AB2-`, `AB3-`, `AB4-` SSID
prefixes and the reported `AXJANUSBWCDEVICE` token remain leads until their
original captures or precise manufacturer documentation can be reviewed.

[Theengs Decoder](https://github.com/theengs/decoder/blob/development/LICENSE.txt)
is especially useful for identifying protocol opportunities across everyday BLE
devices, but its license file is GPL-3.0. AirGuard and Eddystone have Apache-2.0
license files; Nearby Glasses is AGPL-3.0; AXEOFF is MIT. The
[surveillance-signatures aggregator](https://github.com/magikh0e/surveillance-signatures)
is GPL-3.0 and is a discovery source, not an automatically importable database.
No code or bulk tables from these projects were imported. Before any future code
reuse, retain its actual license/notices and check the particular file/data's
provenance, not only the repository badge.

## Platform and schema gaps

- Desktop CoreBluetooth exposes advertised names, service UUIDs and manufacturer/
  service data, but normally uses an opaque identifier instead of the BLE MAC.
  A candidate that needs MAC vendor attribution is therefore backend-dependent.
- ESP32 scans and host backends may omit scan-response fields, truncate AD data,
  or miss extended advertisements. The current M5 scanner has a 62-byte storage
  bound; fitting inside it does not establish extended-advertisement support.
- GATT discovery through a connection is different from observing an advertised
  service. This work adds no probing, pairing or control of detected devices.
- V1 `ble_ad_bytes` is too ambiguous for robust protocol recognition: the schema
  describes raw AD offsets while existing compiled defaults use manufacturer
  offsets. Before Find Hub/BTHome/Eddystone additions, introduce explicit AD type,
  company/service UUID, payload length and payload-relative offset/mask semantics.
  Handle multiple AD sections, truncation and malformed lengths explicitly.
- For Find Hub, `FEAA` alone also matches Eddystone. Google's documented frame
  type (`40`/`41`) and payload size distinguish the protocol candidate; extended
  variants need a genuinely capable receiver. A protocol flag is not evidence
  that this user is being followed.
- RSSI cannot provide a universal distance threshold. Rotation and repeated
  advertisements prevent event counts from being treated as unique-device counts.

## Recommended implementation order

1. Land evidence/confidence separation (#154) and retain all ordinary observations.
2. Add structured BLE AD matching and provenance-bearing data generation so JSON
   packs and compiled Rust defaults cannot silently diverge. Keep optional
   inventory classes separate from the alert policy.
3. Implement BTHome/Eddystone/Find Hub parsers from primary specifications, with
   truncation and wrong-context fixtures before real-device validation.
4. Validate this glasses/equipment candidate pack using known consenting devices
   and same-vendor negatives; promote only the supported models and states.
5. Add a desktop external-decoder adapter for broader radio families such as
   rtl_433, preserving source, receiver, timing and confidence in each event.
