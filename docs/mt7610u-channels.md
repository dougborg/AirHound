# MT7610U passive receive channels

The companion accepts 38 primary channels at 20 MHz:

| Band | Channels | Center frequencies (MHz) |
| --- | --- | --- |
| 2.4 GHz | 1–13 | 2412–2472, in 5 MHz steps |
| Lower 5 GHz | 36, 40, 44, 48, 52, 56, 60, 64 | 5180–5320, in 20 MHz steps |
| Middle 5 GHz | 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144 | 5500–5720, in 20 MHz steps |
| Upper 5 GHz | 149, 153, 157, 161, 165 | 5745–5825, in 20 MHz steps |

This is a hardware tuning allowlist, not a country-specific AP channel plan.
It includes channels used for DFS networks, but implements passive reception
only, not radar detection, AP operation, active probing, or transmit permission.
MAC TX, automatic ACK and beacon transmission remain disabled.

Channel 14, 4.9 GHz/11J channels, wider-channel centers such as 42 and 155,
and channels above 165 are deliberately rejected. No 6 GHz support is claimed.
The default channel selection remains unchanged; opt into broader coverage
with `--channels`. Listening on more channels increases revisit time.

## Independent specification check

Sources checked on 2026-11-19:

- [ALFA AWUS036ACHM product specification](https://www.alfa.com.tw/products/awus036achm)
  identifies MT7610U and 2.4/5 GHz operation. It does not establish sensitivity
  or certify every channel in this implementation.
- [MathWorks WLAN channel and bandwidth table](https://uk.mathworks.com/help/wlan/ug/valid-channel-number-and-bandwidth-combinations.html)
  references IEEE 802.11 Annex E. The selected channel numbers, center
  frequencies and 20 MHz widths match its published entries. Its broader
  table also includes channels our pinned driver tables do not support.
- [Cisco spectrum management channel diagrams](https://www.cisco.com/c/dam/global/en_ca/assets/ciscoconnect/2014/pdfs/spectrum_management_best_practices_in_a_gigabit_wireless_world_jim_florwick.pdf)
  provide an additional primary-channel/width reference.

The radio unit test independently lists all 38 published center frequencies
and rejects wider-channel centers. This validates channel mapping, not
physical tuning accuracy or regulatory certification.

## Linux implementation provenance

Register settings come from
[mt76 revision 22f7ff9be59890cb10cde16cc25a30f78b504f23](https://github.com/openwrt/mt76/tree/22f7ff9be59890cb10cde16cc25a30f78b504f23):

- `mt76x0/initvals_phy.h`: frequency plans, SDM selection, band-switch masks.
- `mt76x0/phy.c`: register ordering, masks and SDM reset sequence.
- `mt76x02_eeprom.c`: per-frequency-range gain selection and fallback.
- `mt76x02_eeprom.h`: EEPROM offsets and field validity.

`channel_tables.rs` keeps the complete selected PLL fields, including the
special SDM plans for channels 153 and 165. Band-switch writes preserve Linux
ordering. RF low/middle/high bands change after channels 64 and 136; EEPROM
gain selection changes after 64 and 128. Missing secondary gain fields (0 or
255) fall back to the low-band field as in Linux.

Check every selected table value against a checkout at the pinned revision:

```sh
python3 drivers/mt7610u/tools/check-channel-tables.py /path/to/mt76
cargo test --manifest-path drivers/mt7610u/Cargo.toml
```

The checker reads the pinned Git object, so uncommitted reference edits cannot
change the comparison. Tests exercise all channels in both directions,
including transitions into and out of SDM mode, EEPROM gain boundaries,
fallbacks, unsupported board bands, and disabled transmit gates.

AirHound's MIT protocol validator independently accepts the same channel
numbers without linking GPL driver code. Older host binaries with the smaller
allowlist must be upgraded before using new channels. The observation format
and its distinction between configured and advertised channel are unchanged.

## Hardware validation

A successful register comparison does not establish reception on every channel.
Record configured-channel and matching advertised-channel beacon counts
separately; off-channel reception is not proof of correct tuning. Quiet
channels need a known AP test before claiming reception support.

Full calibration and sensitivity validation remain pending as described in
[receiver release gates](receiver-release-gates.md). RSSI remains an explicitly
unvalidated estimate. Captures stay local and out of Git.

### Local AWUS036ACHM smoke test, 2026-11-19

A 120.001-second capture visited all 38 channels with 500 ms dwell: 219 hops,
9.711 seconds total retuning, 1,995 decoded beacons and verified disabled
shutdown gates. The updated host capture validator accepted all 1,995 records.
Matching advertised/configured beacons on newly enabled 5 GHz channels were:
100: 115; 132: 95; 149: 23; 153: 40; 157: 95. This includes the SDM path on 153
and successful returns to non-SDM channels. Channel 165's SDM programming was
unit-tested but no beacon was received there in this run.

The environment had no controlled AP per channel. Channels 12/13 heard only
beacons advertising other channels; this does not validate their tuning.
There were 307 malformed/unsupported transfers among 11,164 USB transfers,
counted and rejected by the decoder. This smoke test does not establish
sensitivity, packet-loss rate, full calibration, or cold-start independence
(the firmware was already resident). Raw capture and aggregate summary remain
under the ignored local `drivers/mt7610u/target/validation/expanded-channels-01/`.
