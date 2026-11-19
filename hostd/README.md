# AirHound desktop scanner

`airhound-hostd` records WiFi/BLE observations in a local WiGLE CSV file and
prints matching devices as NDJSON on stdout. macOS supports CoreWLAN WiFi,
CoreBluetooth BLE, and CoreLocation. Linux supports BLE through BlueZ and optional receive-only WiFi capture on an
externally configured monitor-mode interface. Linux location is not implemented.

From the repository root:

```sh
just test-hostd
just clippy-hostd
just run-hostd
```

On macOS, `run-hostd` builds an app bundle with the Location Services usage
descriptions. Grant the applicable Bluetooth and Location Services permissions
when macOS requests them. For a custom output path:

```sh
just bundle-hostd
hostd/AirHound.app/Contents/MacOS/airhound-hostd --output observations.csv
```

On Linux, install the distribution's D-Bus development package (e.g.
`libdbus-1-dev` and `libpcap-dev` on Ubuntu), run BlueZ, and ensure your user can
access the adapter. WiFi capture is enabled with `--wifi-interface wlan0mon`;
configure monitor mode and the channel externally, and grant capture access.
Without this option, Linux scans BLE only. `--no-ble` requires a WiFi interface.
The binary is `hostd/target/release/airhound-hostd` after `just build-hostd`.

Every observation is a CSV row, including unmatched devices. Repeated sightings
are retained. The default filename includes the start time; an explicit output
path must not exist unless `--overwrite` is also given. CSV output is flushed every five seconds, even
when scanners are idle, and the queued observations are drained on Ctrl-C.
NDJSON commands such as `{"cmd":"stop"}`, `{"cmd":"start"}` and
`{"cmd":"status"}` are accepted on stdin.

CoreWLAN and btleplug expose scan results rather than complete radio frames.
Raw BLE advertisements and macOS WiFi information elements are unavailable.
Linux monitor capture supplies radiotap metadata and raw WiFi frames for the
shared WiFi and Remote ID parsers; decoded Remote ID is emitted as NDJSON.
Capture uses a bounded queue and drops events when the consumer cannot keep up.
Platform-provided BLE addresses may be synthetic rather than over-the-air MACs.
WiFi frequency conversion currently covers 2.4/5 GHz channel numbering; 6 GHz
band identification is not implemented. Location fields remain empty when no
recent valid fix is available.

Tests exercise session startup, command input, conversion, event output, CSV
escaping, error propagation and shutdown without starting a radio scan. Linux
capture tests use offline PCAP files. Both platforms enforce 81% production-line
coverage, including adapter code; test bodies are excluded. Permission prompts, live scans, and location delivery
still require hardware testing on the target OS.
