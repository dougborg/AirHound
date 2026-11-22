# External GNSS for desktop wardriving

`airhound-hostd --gpsd 127.0.0.1:2947 --output observations.csv` attaches fresh
receiver positions to WiGLE observation rows on macOS and Linux. It works with
native scanning and external scanner sessions. Positions describe the **scanner**,
not the detected device: an observation is evidence of reception from a location,
not a precise location of the transmitter.

Source-built multi-receiver sessions and the shared service can also select gpsd
in the session plan. Live NDJSON/SQLite observations then retain the receiver's
position and fix provenance; see below. This is not in the published 0.2.0 preview
archives.

## Save a GPS setup without a terminal launch each session

The native app supports two GPSD lifecycles. **Connect to an existing server**
keeps the external-server behavior. **Start GPSD with this recording** lets the
Rust service own a foreground, read-only helper for that recording.

For an attached USB GPS and an already installed gpsd:

1. Under Available devices, expand the GPS and choose **Use for Location**.
2. In Location, enable **Start GPSD with this recording** and choose the installed
   `gpsd` executable. On this tested Apple Silicon/Homebrew setup it is
   `/opt/homebrew/sbin/gpsd`. The executable is not bundled or installed by AirHound.
3. Confirm the explicit GPS device path and unused local endpoint (the form uses
   `127.0.0.1:12947`, separate from the conventional external-server port).
4. Save the setup. On later sessions, load/review/apply it and Start Recording.
   No separate terminal daemon command is needed. Changed USB paths still require
   explicit review; AirHound does not silently choose another GPS.

The saved provider is:

```json
{"kind":"managed_gpsd","endpoint":"127.0.0.1:12947","device":"/dev/cu.usbmodem112201","executable":"/opt/homebrew/sbin/gpsd"}
```

Validation only checks configuration and local file metadata. It never opens the
GPS or starts the daemon. Clients check `status.location_providers` for
`managed_gpsd` before offering this mode. Loading a saved setup does not start it.

At recording start the service takes the shared per-device AirHound serial lease
(the same lease used by current Sniffle and M5 sources), checks the
endpoint is unused, and starts only the selected helper with `-N -b -n -S PORT
DEVICE`. It verifies the exact child owns the IPv4 localhost listener before
sending WATCH or accepting location data. gpsd handles serial exclusivity and
framing; capture-plan validation also rejects a capture receiver sharing that
GPS endpoint, including macOS cu/tty aliases. A busy endpoint or AirHound lease
fails explicitly. AirHound never adopts or terminates a preexisting gpsd.

Finish/shutdown stops and reaps the owned helper; closing the GUI leaves it with
the service-owned recording. Child exit clears current fixes and daemon metadata
before at most three total attempts, spaced two seconds apart. Exhausting that
budget leaves an explicit provider error; finish and restart the recording after
correcting the problem. Process-wide force termination can leave a child behind;
a later run reports the occupied endpoint rather than taking ownership of it.
No login item or persistent OS service is installed.

Health reports the version from the actual gpsd VERSION message. This is **host
software**, not firmware on the GPS receiver. A running daemon also does not imply
a valid satellite fix. Existing freshness, device-binding and rig-position rules
apply to both modes.

The fixed `-b` flag prevents receiver writes; `-N` keeps the child in the foreground.
See the [gpsd options](https://gpsd.io/gpsd.html) and
[device ownership FAQ](https://gpsd.gitlab.io/gpsd/faq.html).

## Shared service and session plans

Select location separately from capture receivers:

```json
{
  "version": 1,
  "location": {
    "kind": "gpsd",
    "endpoint": "127.0.0.1:2947",
    "device": "/dev/cu.usbmodem112201"
  },
  "receivers": [
    {"id": "ble", "source": {"kind": "native_ble"}}
  ]
}
```

Use this plan with `--session-config` or the shared client's `start
--session-config` operation. The endpoint must be a concrete IPv4/IPv6 address
and nonzero port, for example `127.0.0.1:2947` or `[::1]:2947`; validation never
resolves a hostname or connects to the server. gpsd must already be running.
AirHound does not start it, open the GNSS serial device, or configure the receiver.
Do not select the same GNSS port as a capture source. For a loopback gpsd endpoint
and an explicit `/dev/` path, validation reserves that serial resource against
capture sources, including macOS callout/dial-in aliases. It cannot establish
ownership of a remote daemon's devices or an unspecified first-device selection.

`device` is an optional gpsd device identifier. Omit it to bind to the first
reporting device for this session, or set it to the exact path reported by gpsd.
The provider retains that binding across reconnects. With a configured provider,
an unavailable or stale fix does not trigger a fallback to another device or to
CoreLocation. Capture can continue without coordinates; `capture-status` exposes
the reason under `receiver_health.location`. Pause stops observation ingestion;
the gpsd connection can continue tracking fixes. Finish/shutdown closes it.

`location` is optional and existing plans retain platform behavior. An explicitly
configured location provider is rejected for replay-only plans. In a mixed live
and replay session, replay records never receive the host's current fix. Using
both a plan's `location` and the CLI's `--gpsd` is rejected rather than silently
overriding one. When CLI gpsd flags are used with a session plan, the selected
provider is also included in the durable recording configuration.

Clients can check `status.location_providers` for `gpsd`. Passive discovery's GPS
candidate has a `location_configuration_hint` intended for the plan's `location`
field, not its receiver list. Its suggested device path is unverified; confirm the
gpsd endpoint and selected device before recording.

### Retained location evidence

With a fresh explicit gpsd fix, live records contain `receiver_location`:

- `position_of: "receiver_rig"`, `association: "host_processing_time"`, and
  `capture_time_synchronized: false` distinguish the host/rig's position from a
  transmitter position or a position synchronized with the producer's capture.
- `source`, `device`, `fix_time`, `associated_at`, `fix_age_ms`,
  `host_fix_age_ms`, and `max_age_ms` retain provenance and freshness bounds.
- `latitude`/`longitude` are WGS84 degrees; `altitude_msl_m` and
  `horizontal_accuracy_m` preserve their original units and may be null.

The backend samples this evidence when it processes an observation. Queueing,
serial buffering, M5 boot-relative timestamps, scan batches, and unsynchronized
producer clocks prevent a claim that this is the position at RF capture time.
It is not interpolation, triangulation, or a transmitter-location estimate.
Absent or stale fixes omit the field. Existing observation timestamps and remote
device coordinates remain intact. SQLite retains the same JSON evidence without a
new database schema; a future indexed location model needs its own migration.

Provider health reports the gpsd daemon version and protocol from its actual
VERSION message. This is host software identity, not the attached receiver's
model or firmware; unknown receiver firmware stays unknown. Disconnect clears
the current daemon metadata and fix until a new connection reports them.

## Receiver setup

Install gpsd using your platform's package manager, then run it in the foreground
on a dedicated terminal. The example uses the currently observed macOS port;
use the actual receiver port (prefer a stable `/dev/serial/by-id/` path on Linux):

```sh
gpsd -N -b -n /dev/cu.usbmodem112201
```

`-b` is gpsd's read-only mode: it prevents receiver configuration writes. Do not
share the serial port with another reader. gpsd owns serial framing, NMEA checksum
validation, receiver protocols, and constellation handling. AirHound only sends
a JSON WATCH request to gpsd; it never sends receiver configuration commands.
Keep gpsd bound to localhost (its default); the protocol has no authentication or
encryption. AirHound accepts gpsd protocol major versions 3 (legacy) and 16 (gpsd 3.27.5).

```sh
airhound-hostd --gpsd 127.0.0.1:2947 \
  --gpsd-device /dev/cu.usbmodem112201 --output observations.csv
```

`--gpsd-device` is optional. Without it, the first reporting TPV device becomes the
source for the entire session, including reconnects. It never silently switches
to another receiver. With an explicit gpsd source there is **no fallback** to
CoreLocation when GNSS is unavailable. macOS still initializes CoreLocation for
the separate permission needed by Wi-Fi scanning. Without `--gpsd`, existing
CoreLocation behavior on macOS is retained; Linux has no default location source.

The attached receiver enumerated as `u_blox GNSS receiver` (USB `1546:01a8`).
USB identifiers alone do not establish the receiver model. Initial passive NMEA
reads had no fix; a subsequent check on 2026-11-20 reported a valid 3D fix,
five satellites used, eight in view, and HDOP 3.63 (dimensionless).

A live desktop check with Homebrew gpsd 3.27.5 then confirmed fresh fixes in
AirHound health output and clearing of the fix when gpsd stopped. This exposed
and corrected an overly restrictive protocol-version check: this gpsd reports
API major 16, while the upstream JSON documentation's old VERSION example uses 3.
Both are accepted; unknown major versions are rejected. Transport errors are
logged to distinguish protocol failures from an unavailable receiver.

After granting macOS Location Services and Bluetooth permissions, a second live
capture on 2026-11-20 validated the receiver → gpsd → AirHound → WiGLE CSV path:

- 3,286 observation rows: 2,002 Wi-Fi and 1,284 BLE.
- Nine startup rows had blank locations while awaiting the first accepted fix.
- 2,539 consecutive rows had valid coordinates, MSL altitude and reported
  horizontal accuracy in meters.
- Stopping gpsd while scanning continued produced 738 subsequent rows with blank
  location fields; no cached position continued to be attached after disconnect.
- Both scanners reported running. The capture and temporary gpsd process were
  stopped after the check; raw identifiers and positions remain private local data.

This validates stationary live geotagging and daemon-disconnect handling, not
moving-route accuracy, physical receiver unplug/replug, or capture-time position
association. GPS coordinates describe the receiver location, not the transmitter.

## Validity and field semantics

- A fix requires a complete TPV message with mode 2 or 3, finite in-range latitude
  and longitude, and a parseable UTC timestamp. Partial TPV messages are not merged
  with older fields: this conservative client can omit positions from devices
  that split their fix across messages. gpsd dead-reckoning-only, time-only and
  simulated statuses are rejected; combined GNSS/dead-reckoning status is allowed.
- Receiver timestamps must be at most five seconds old, with up to two seconds
  of future clock tolerance. Local monotonic age is also bounded at five seconds.
  Keep the desktop clock synchronized. Duplicate or out-of-order timestamps do
  not refresh a fix, including after TCP reconnect.
- No-fix, invalid, selected-device removal, and disconnection clear the current
  fix. Reconnect is automatic after two seconds. Response lines are bounded at
  16 KiB, and shutdown cancels idle connections.
- WiGLE altitude is `altMSL` in meters and only present for a 3D fix. Ellipsoid
  altitude (`altHAE`) and deprecated ambiguous `alt` are never substituted.
  Accuracy uses gpsd `eph` in meters when available; its confidence convention is
  receiver-dependent. **HDOP is dimensionless and is never converted to meters.**
  Missing altitude or accuracy stays blank, as do all coordinates when no valid
  fix exists. Genuine valid coordinates at 0,0 remain valid.
- Event health includes gpsd source, selected device, fix UTC time, freshness policy, state,
  altitude datum and accuracy provenance, without exposing coordinates. WiGLE
  rows retain their observation timestamps; they do not contain the GNSS fix
  timestamp. Association currently happens at **processing time**, so scan batch
  latency can offset the position of a moving scanner. Capture-time interpolation
  remains follow-up work. Source-built live NDJSON/SQLite records
  retain the processing-time association and per-fix provenance described above.

WiGLE CSV contains precise receiver locations and wireless identifiers. Keep it
local unless you intentionally choose to share it. AirHound does not upload it.

## Validation and remaining roadmap

Automated replay tests cover gpsd validity, stale/future/duplicate/out-of-order
fixes, optional field semantics, multiple-device isolation, reconnect state,
bounded TCP input, shutdown, and the production WiGLE writer. These are JSON
client tests; they do not claim to revalidate gpsd's NMEA parser. Hardware fix
acquisition, live CSV geotagging and gpsd disconnect clearing have been checked.
Physical unplug/replug and moving-route field accuracy remain unvalidated.

The service integration also has a mock-gpsd → capture actor → durable SQLite
regression covering receiver-location association, actual daemon version health,
WATCH-only requests and shutdown. Unit checks reject malformed endpoints,
replay-only location plans, stale evidence and accidental location attachment to
replay records. These mock tests are separate from the bounded live acceptance below.

This is the desktop provider slice of [#79](https://github.com/dougborg/AirHound/issues/79).
Portable location types and firmware integration remain in
[#28](https://github.com/dougborg/AirHound/issues/28); hardware acceptance remains in
[#141](https://github.com/dougborg/AirHound/issues/141).

References: [gpsd JSON protocol](https://gpsd.io/gpsd_json.html),
[gpsd daemon and read-only mode](https://gpsd.io/gpsd.html),
[u-blox receiver protocol reference](https://content.u-blox.com/sites/default/files/products/documents/u-blox6_ReceiverDescrProtSpec_%28GPS.G6-SW-10018%29_Public.pdf).
The protocol reference explains NMEA validity; it does not identify the attached model.

## Service recording acceptance — 2026-11-22

Two private service runs used the attached u-blox GNSS receiver through read-only
gpsd and CatSniffer/Sniffle on BLE channel 37/1M. No native WiFi or Bluetooth
receiver, default service, or GUI recording was used. Production code was the
GNSS service integration plus the native-WiFi-only CoreLocation initialization
change (`018eea9`); this source change is not in the published preview.

| Check | Result |
| --- | --- |
| 60-second steady run | 4,520 BLE packet observations, all with retained receiver-location evidence; finished normally |
| Freshness | GNSS fix age 16–1,020 ms; local monotonic age 0–1,004 ms; six of six sampled provider states reported a fix |
| Version evidence | Actual Sniffle 1.11.0/API 0 and gpsd 3.27.5/protocol 16.1; GNSS receiver firmware remains unknown |
| Loss counters | CatSniffer reported zero host drops and zero reconnects; RF loss remains unknown |
| Separate 30-second provider-loss run | 2,441 packet observations total; 842 geotagged before provider loss. With a one-second disconnect guard, 1,476 later packet observations were retained with no receiver-location field |
| Provider-loss state | Health changed from fix to disconnected; current gpsd daemon metadata cleared. Capture continued and finished normally |
| Cleanup | Each private service and gpsd process exited with status 0; both capture and GNSS endpoints were released |

Both recordings had zero invalid receiver-location association labels. The
provider-loss test stopped only its own gpsd process, not the physical receiver;
it does not establish USB unplug/replug, moving-route accuracy, or capture-time
synchronization. Raw positions, wireless identifiers, plans, SQLite databases and
logs stay in private local test directories. Only aggregate evidence is retained
in this repository.

### Managed provider acceptance (2026-11-22)

A private service saved and loaded a managed GPS + CatSniffer setup, then started
and finished a 20-second recording using the installed gpsd executable and stable
Sniffle helper assets. It retained 1,356 BLE packet observations, including 1,346
with fresh receiver/rig location (ten startup packets were correctly unlocated).
Health reported gpsd 3.27.5/protocol 16.1 and Sniffle firmware 1.11.0/API 0.
Explicit Finish returned `finished`; the owned helper and private service exited,
and both serial endpoints were released. A prior 30-second run also verified
service-shutdown cleanup. Neither run used native radios or changed the desktop's
installed service/default recordings. This verifies the Rust lifecycle; the new
native controls still need an actual-window usability check after integration.
