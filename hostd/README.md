# AirHound desktop scanner

`airhound-hostd` records WiFi/BLE observations in a local WiGLE CSV file and
prints matching devices as NDJSON on stdout. macOS supports CoreWLAN WiFi,
CoreBluetooth BLE, and CoreLocation. Linux currently supports BLE through BlueZ;
WiFi monitor mode and location providers are not implemented there.

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
`libdbus-1-dev` on Ubuntu), run BlueZ, and ensure your user can access the adapter.
The binary is `hostd/target/release/airhound-hostd` after `just build-hostd`.

Every observation is a CSV row, including unmatched devices. Repeated sightings
are retained. The default filename includes the start time; an explicit output
path is created or overwritten. CSV output is flushed every five seconds, even
when scanners are idle, and the queued observations are drained on Ctrl-C.
NDJSON commands such as `{"cmd":"stop"}`, `{"cmd":"start"}` and
`{"cmd":"status"}` are accepted on stdin.

Desktop APIs expose scan results rather than complete radio frames. Raw BLE
advertisements and WiFi information elements are not supplied by these adapters,
so firmware-style raw-byte signatures and Remote ID decoding are not provided.
Platform-provided BLE addresses may be synthetic rather than over-the-air MACs.
WiFi frequency conversion currently covers 2.4/5 GHz channel numbering; 6 GHz
band identification is not implemented. Location fields remain empty when no
recent valid fix is available.

Tests exercise conversion, CSV escaping, and idle/shutdown flushing without
starting a radio scan. Permission prompts, live scans, and location delivery
still require hardware testing on the target OS.
