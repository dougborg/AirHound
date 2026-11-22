# Managed receiver helpers

AirHound can keep explicitly selected local Sniffle and ALFA helper assets in a
private, durable installation. The Rust backend reads the installed manifest when
it builds receiver configuration hints. Saved profiles retain exact immutable
paths, so installing a newer bundle does not silently change an existing plan.

This is local setup for the hardware preview. It does not download software,
flash devices, or provide a public redistributable package. Release packaging,
updates, signing and runtime bundling remain tracked in #268.

## Prepare local assets

`scripts/stage-local-receiver-helpers.py --help` lists the required paths. Provide:

- NCC Group Sniffle v1.11.0 source at revision
  `18252f2c5a05f9ad906865b776652e31c61471c4` and pyserial 3.5's installed package.
- A stable Python base interpreter, outside temporary directories. The interpreter
  is executed explicitly to report its version and framework prefix.
- The chosen ALFA executable, its local source checkout, and the reference
  `mt7610e.bin` image (SHA-256
  `538d08c08f8fb5aa05cacf34e49aeb307e8b2dc094407fc655fbf3580c893953`).
- Sniffle/ALFA notices, the [pyserial 3.5 license](https://github.com/pyserial/pyserial/blob/v3.5/LICENSE.txt),
  and the libusb license. The script copies notices and source into the bundle.

The script has no network or device access. It refuses tracked source changes,
source symlinks and Python native extensions that would need a different packaging
review. This bundle supports Sniffle's serial hardware path; its optional SDR
modules require additional dependencies and are not covered by this setup.

The manifest records every copied file's size and SHA-256, executable mode,
platform/architecture, component versions, source revisions and external Python
runtime. These hashes provide integrity and provenance of the selected files;
they are not signatures or proof that a local binary was built from the supplied
source snapshot. ALFA's firmware redistribution notice remains in the bundle;
this local import does not establish permission for a public firmware release.

## Install and check

Build `airhound-helper-setup` with the desktop workspace. The default installation
is `~/Library/Application Support/AirHound/helpers` on macOS and
`$XDG_DATA_HOME/airhound/helpers` (normally `~/.local/share/airhound/helpers`) on
Linux. Its AirHound parent directory must already be owned by the current user
and have mode `0700`. A custom `--root` is available for tests; discovery uses the
standard per-user location.

```sh
cargo build --manifest-path hostd/Cargo.toml --bin airhound-helper-setup
hostd/target/debug/airhound-helper-setup install --bundle /absolute/staged-bundle
hostd/target/debug/airhound-helper-setup verify
hostd/target/debug/airhound-helper-setup status
```

An existing installation requires `install --replace`. Complete bundles live in
`bundles/<manifest-sha256>`; the current pointer changes only after verification.
Old bundle directories remain available for saved profiles and active captures.
Neither installation nor verification executes helper code or opens hardware.
Interrupted staging may leave a `.pending-<pid>` directory; it is never advertised
as an installed bundle. Remove it only after confirming its installer has exited.

The store accepts only bounded manifests and regular, nonsymlink files, with
explicit relative paths. Files are private (`0600`, or `0700` for executable
helpers); installation uses a nonblocking lock and atomic publication. The
128 MiB/2,048-file limits bound imports. Do not edit installed bundle files;
`verify` detects size/hash/mode changes and changes to the external interpreter.

Run the separate **explicit runtime check** using the installed bundle path from
`status`:

```sh
python3 scripts/check-installed-sniffle.py --bundle /absolute/helpers/bundles/ID
```

This intentionally executes the selected installed code with isolated Python
(`-I -B`). It imports pyserial and Sniffle's hardware/decoder/PCAP modules, checks
that they resolve inside the installed module directory, and runs helper
`--help`. It never opens a serial port. Python's framework and standard library
remain external dependencies; changing/removing Xcode or that Python installation
can require restaging. An interpreter hash does not fingerprint every framework
or standard-library file. Current serial dependencies contain no native extension;
this is not an assumption that arbitrary Python environments are relocatable.

## Discovery and verification states

Service `status.managed_helpers` exposes bundle provenance and resolved defaults.
`devices.list` uses those defaults for configuration hints. Discovery only reads
the bounded manifest and ordinary file metadata; it never imports Python, hashes
all installed assets, queries a device, or starts a capture. Its
`validation_scope: manifest_only`, `runtime_verified: false` and
`rf_verified: false` deliberately do not promote previous setup checks to current
hardware readiness. Explicit user settings and saved profiles are not rewritten.

A configured bundle is not proof that a connected board runs compatible firmware.
Use the existing explicit Sniffle identification operation and capture acceptance
checks. ALFA's supplied firmware file is also distinct from evidence about the
image actually running on its MCU.

## Local acceptance evidence

On November 22, 2026, an installed bundle of 100 files (2.8 MB) passed explicit
integrity verification, isolated pyserial/Sniffle imports, and helper `--help`.
All four imported module paths were under the stable managed bundle; the runtime
was Xcode's installed Python 3.9.6 framework. A separately authorized bounded
CatSniffer `--identify-only` query using those installed paths returned firmware
1.11.0 / API 0 and exited successfully. No temporary directory was used as a
runtime dependency. This check did not start capture or alter the running service.

The subsequent 102-file (7.9 MB) bundle contains the separately hardware-validated
ALFA helper built at `f292753d78ec41263c1c7c19a26ef74e705278c6` (binary SHA-256
`84a8d071145d46c8d5a3ad13a45a3cfd150d3b9e67ef3a9bd78ba8e4ce1e8561`).
The installed copy passed its USB-free `build-info` check and the Sniffle import
check again. Its macOS dynamic dependencies are OS libraries/frameworks. The
prior bundle remains installed. Final multi-receiver acceptance uses the current
managed paths; this offline check alone does not establish RF readiness.
