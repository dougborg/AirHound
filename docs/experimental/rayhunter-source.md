# Experimental Rayhunter receiver (implementation contract)

This branch depends on the installed storage-recovery backend `d8ad552`.
The cellular adapter is MIT; Qualcomm/RRC decoding stays in a separately built
GPL Rayhunter helper. No decoder code is linked into AirHound.

Explicit receiver configuration uses `kind: rayhunter`, a loopback WebSocket
`endpoint` ending in `/raw-diag`, and an absolute `decoder` executable path.
Direct endpoint mode uses a separately prepared forward. Alternatively, omit
`endpoint` and provide `adb: {"executable":"/absolute/path/to/adb","serial":"USB_SERIAL"}`.
The backend advertises `managed_adb_usb` in `rayhunter_transports`. It validates
the selected authorized USB transport and owns a dynamically allocated local
forward to device port 8082 for each subscription. Supplying both modes is rejected.
Discovery/validation never installs software,
changes modem logging, starts a recording, or selects an arbitrary attached device.

Managed ADB pins the executable hash for the recording and records the selected
USB identity, returned local port and remote port with source provenance. Pause,
Finish and reconnect close the helper and remove only the returned, rechecked
forward tuple; Resume allocates a fresh one. The cooperative per-user lease does
not establish global DIAG exclusivity. A lost allocation response is an explicit
unknown ownership failure, never permission to remove a guessed forward.
Cancellation lets the bounded owner finish allocation and cleanup; normal shutdown
awaits its result. Process crashes cannot guarantee remote cleanup, and ADB's
list/remove interface is not atomic against unrelated actors replacing a tuple.
No server kill, remove-all, network-device selection or modem command is used.
The command contract follows the [ADB manual](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/user/adb.1.md).

The helper emits versioned, bounded JSON IPC. A hello identifies the raw protocol,
reader epoch and initial sequence. Exact device-read frames are chunked below the
shared record size limit; SHA-256, offset/length and source metadata permit offline
reconstruction. Decoded LTE serving/neighbor measurements and SIB1 cell identities
reference a complete raw frame and message index. Unsupported messages stay in raw
evidence; decode errors and upstream sequence gaps are separate records. A cell is
not a physical tower position, and a modem observation is not an IMSI-catcher finding.

The host records helper executable hash, connection generation, host monotonic and
wall arrival, source read clocks, epoch, recording ID and raw modem timestamp. Source
wall/modem clocks remain unvalidated. Independent GNSS is receiver location at host
ingress, not RF capture time; no location is fabricated when unavailable. No WiGLE
upload or Rayhunter GPS forwarding is part of this slice.

Each helper record snapshots qualified GNSS evidence before queueing, preserving
the fix time, age, receiver source and uncertainty. An unavailable or stale fix
is explicitly null. Later filter processing never replaces that snapshot with a
newer fix, even after queue backpressure.

Pause closes the owned subscription/helper. Resume opens a new bounded connection;
an explicit `subscription_pause_resume` discontinuity precedes the resumed
records, distinct from `transport_disconnect`. Its lost-read count remains unknown
and it makes no RF-loss claim. Finish closes/reaps only that helper. Modem participation
and prototype recording continue independently. Transport retries are bounded and
new connections carry distinct generations even if the upstream epoch is unchanged.
All queues/records have fixed limits. Raw evidence must precede derived observations;
unknown/invalid IPC fails capture rather than silently manufacturing provenance.

Persistent prototype deployment is a separate reviewed action. Successful tests leave
the prototype installed as requested; verified official rollback is retained and used
only for necessary recovery or an explicit request. Historical restoration receipts
remain unchanged.

## Build and review scope

The host helper lives in the GPL Rayhunter fork's `airhound-source` package, built
with `cargo build --locked -p rayhunter-airhound-source`. Select that executable
explicitly; it never opens `/dev/diag` itself. Its raw endpoint is subscription-only.
AirHound exposes the new source in `status.session_sources`; older backends do not
enable the setup button. No automatic device/forward discovery is claimed here.

IPC lines are at most 96 KiB; each exact raw read is at most 256 KiB, emitted in
32 KiB chunks. The host verifies the complete SHA-256 before admitting derived
cells. The final chunk marks `frame_complete`; earlier chunks alone are not a
complete frame. A diagnostic message index is bounded below 4096. Source queues
remain bounded and use backpressure; upstream eviction is a sequence-gap record.
Each connection has an eight-second IPC deadline; up to three reconnects use
one/two/three-second delays. Pause/Finish interrupt blocked reads/queue sends and
kill/reap only the owned helper (three-second reap bound). Cancellation uses the
runtime's kill-on-drop/reaper. There is no modem stop or diagnostic reset command.

The first decoded fields are LTE serving/neighbor signal measurements and broadcast
SIB1 PLMN/TAC/ECI. EARFCN is not truncated to GSMTAP's narrower field. MNC remains a
digit string. An EARFCN/PCI-only measurement is explicitly partial; observations
are not joined across frames into a supposedly time-qualified cell identity.
The helper executable's file hash is pinned for the recording and rechecked before
Resume/reconnect. Upstream version/epoch fields are reported provenance, not remote
cryptographic attestation. Host ingress times describe helper delivery, not RF arrival.

Validation so far uses synthetic protocol/decoder fixtures, private child processes,
private storage and a temporary loopback server. It establishes no live Orbic,
physical RF loss, GPS clock alignment, or installed native-button acceptance.
