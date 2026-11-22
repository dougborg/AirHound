# WiFi receive frequency and band

[Documentation index](README.md) · [Host daemon](../hostd/README.md) · [Device discovery](device-discovery.md)

Desktop `wifi_observation` records retain every parsed WiFi observation, including
ordinary APs and clients. Signature matches also produce the legacy `wifi` record
and detection evidence. Both desktop record types include these additive fields:

```json
{"ch":1,"frequency_mhz":5955,"band":"6GHz"}
```

`frequency_mhz` is the receive frequency reported by the backend. `band` is derived
from that frequency (`2.4GHz`, `5GHz`, or `6GHz`). A channel number alone cannot
identify a band: channel 1 can mean 2412 MHz or 5955 MHz. These values describe
reception, not the channel advertised inside a beacon. Capture width is not yet
represented and must not be inferred from frequency or band.

When the backend supplies only a channel, both new fields are `null`. Older
firmware records omit the fields; consumers must treat absence and null as unknown.
The existing `ch` key remains, with 0 meaning unknown. WiGLE CSV preserves an
explicit frequency and leaves the Frequency cell empty when it is unknown; it no
longer invents a frequency from a desktop channel number.

The current macOS `wifi_scan` dependency exposes no band or explicit frequency.
Its `get_frequency()` method guesses from the channel, so AirHound does not use it.
CoreWLAN observations therefore retain their channel but report unknown band and
frequency. This is a metadata limitation, not evidence of a missing network.

Radiotap capture preserves the packet's reported frequency. Conversion recognizes
the 2.4 GHz raster, the 4.9/5 GHz raster, and 6 GHz through 7115 MHz, including the
2484 MHz/channel 14 and 5935 MHz/channel 2 exceptions. Channel assignments were
cross-checked against [Linux's IEEE 802.11 frequency conversion reference](https://github.com/torvalds/linux/blob/master/net/wireless/util.c).
Only numeric protocol assignments were consulted; no driver implementation was
copied. This conversion is **not a regulatory allowlist** or a statement that a
receiver supports a given frequency or width. Unsupported frequencies and malformed
radiotap records do not produce observations. Bad FCS frames remain rejected.

## New receiver preparation

Passive inventory recognizes `0e8d:7961` (MT7921U / ALFA AWUS036AXML) and
`0846:9072` (MT7925U / Netgear A9000) as experimental, unverified candidates.
The ID evidence comes from the pinned [Network-Weather hardware table](https://github.com/Network-Weather/mt76-usb-macos/blob/01189feb42f67bfc6d4c36dbe22ddb82d2fe832a/README.md#supported-hardware).
Discovery does not claim a USB interface, load firmware or return a working source
configuration. Neither device has been validated with AirHound.

[#235](https://github.com/dougborg/AirHound/issues/235) tracks WiFi PCAP replay, the
separate live helper, firmware prerequisites, topology-safe selection, and hardware
acceptance. Classic radiotap captures can be [replayed offline](wifi-pcap-replay.md); this
does not start a live receiver. The existing MT7610U helper is
a different driver and cannot activate these adapters. Supporting 6 GHz metadata
in Linux radiotap ingestion does not establish an AXML/A9000 capture backend.
