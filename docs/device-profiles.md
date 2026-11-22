# Saved receiver setups

A profile saves a complete session plan on the backend host, including helper,
Python, firmware, replay, and serial paths. CLI, native, TUI, and MCP clients can
use the same service API. Saving or loading a profile never starts capture,
executes a helper, opens a receiver, or changes the active recording.

This is source-build functionality, not a feature of the published 0.2.0 preview
archives. Native presentation is tracked separately from the shared backend API.

## CLI

With a local service running, save a plan that passes configuration checks:

```sh
airhound-hostd client --socket /absolute/private/airhound.sock profiles save \
  --id car-kit --name 'Car kit' --session-config /absolute/path/session.json
airhound-hostd client --socket /absolute/private/airhound.sock profiles list
airhound-hostd client --socket /absolute/private/airhound.sock profiles load --id car-kit
```

A save to an existing ID fails unless `--replace` is explicitly supplied. Remove
a profile with `profiles delete --id car-kit`. IDs contain 1–64 ASCII letters,
digits, hyphens, or underscores; display names can contain spaces. Profiles are
scoped to the service socket's parent directory, not the client machine. For the
native app's default service, they live in
`~/Library/Application Support/AirHound/profiles/` and survive service restarts.

Load returns `profile.config` for the client to review and apply to its plan
editor. It does not start that plan. Run ordinary validation/start commands after
reviewing the current devices and any reported missing resources.

## What validation means

Save uses the canonical session configuration validator, then inspects local
resource metadata. Helper executables, helper scripts, firmware files and replay
files must have absolute paths and the expected file types; executables need an
execute bit. Optional Sniffle module paths must be directories. These checks do
not run the files, parse replay contents, prove dependencies are installed, test
access to a device, or establish RF reception.

Load rechecks resource metadata and passive inventory. A removed helper is
reported as missing while the stored plan remains available to repair. The
response separates `validation` (session structure and platform checks),
`resources` (file metadata), and `bindings` (current endpoint presence). Neither
configuration validation nor a matching endpoint is a radio acceptance test:
`reception` remains `not_tested`.

Serial and network bindings retain the selected endpoint and inventory IDs seen
at save time. A matching serial path or USB topology can refer to replacement
hardware, so it is shown as `present_requires_identity_review`, not as a verified
physical identity. A changed endpoint mapping, missing device or multiple matches
has its own state. The backend never substitutes another device or rewrites the
saved endpoint. Native system-managed sources are identified as such. ALFA helper
USB selectors and explicitly selected OS BLE adapters require review where passive
inventory cannot correlate their selector namespace reliably.

Saving a plan does not remember a transient firmware identification as permanent
truth. The existing live owner or an explicit protocol query supplies current
firmware information; see [device discovery](device-discovery.md).

## Shared API

Clients check `status.methods` for these methods before showing profile actions:

| Method | Parameters | Result |
| --- | --- | --- |
| `profiles.list` | `{}` | `{schema_version, profiles:[{id,name,saved_at_ms,receiver_count,schema_version,validation_scope}]}` |
| `profiles.save` | `{id,name,config,replace:false}` | `{schema_version,profile,validation,resources,reception}` |
| `profiles.load` | `{id}` | `{schema_version,profile,validation,resources,bindings,inventory_diagnostics,reception,applied:false}` |
| `profiles.delete` | `{id}` | `{schema_version,deleted:id}` |

`profile` contains `schema_version:1`, `id`, `name`, `saved_at_ms`, `config`,
`bindings`, and `validation_scope:"configuration_only"`. A binding includes
`receiver_id`, `source_kind`, `selector`, `device_ids`, and `identity_scope`.
Load's `bindings.checks` adds a state and `physical_identity_verified:false` for
each receiver. `bindings.automatic_rebinding` is always false. Clients must not
collapse these checks into a claim that the receiver is RF-ready.

## Persistence and schema changes

The backend creates an owned directory with mode 0700 and regular files with mode
0600. It rejects symlink stores/files, unsafe permissions, nonregular files and
path-like IDs. A nonblocking store lock serializes writes across service instances;
a busy response can be retried. Saves use a private staging file, file sync,
atomic publication and directory sync. Interrupted staging files are cleaned on
the next open under the lock. Replacement requires an explicit flag.

Limits are 64 profiles per store, 64 KiB per session configuration and 128 KiB per
profile file. A malformed or oversized profile makes listing fail explicitly;
it is not silently omitted. Explicit deletion can remove a corrupt or unknown
schema profile without reading it. Unknown schema versions are rejected without
modification. A future migration must explicitly recognize the old version and
atomically replace it after validation; there is no best-effort field guessing or
automatic downgrade. Profiles are separate from the recording database and its
migrations.

## Backend version information

`status.build` reports this backend executable's component, Cargo package version,
Git revision, exact source tag (when available), tracked-source modification state,
control protocol version, and executable path. `display_version` and the existing
handshake `build_version` use the source tag or Git revision, rather than treating
the currently independent host package version as an AirHound release. Builds
without source metadata report it as unknown. This identifies the backend, not
the GUI bundle, device firmware, permission status, or an RF capability.

The current build stamp tracks Git-managed source changes. Untracked files and
external helper versions are not covered by that stamp; receiver identity and
helper setup remain separate evidence.
