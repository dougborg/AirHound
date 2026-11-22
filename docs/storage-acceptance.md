# Desktop storage acceptance

Status: work in progress, 2026-11-22 UTC. These checks cover the SQLite/service
implementation on the storage feature branch; they do not establish general RF
capture completeness or readiness of every listed device.

| Stage | Evidence | Result |
| --- | --- | --- |
| Offline capture | Single and dual radiotap replay; emitted observations and detection evidence compared with stored JSON, source identities and counts | Passed |
| Durable lifecycle | Controlled receiver exercises cancelled pause request, repeated pause, resume, wrong-run finish, drain and cleanup; receiver failure persists failed state | Passed |
| Shared clients | Real CLI processes start and finish through separate service connections; cursor history queries, request-ID retries and service restart | Passed |
| Single live receiver | ALFA AWUS036ACHM/MT7610U, explicit USB selector, existing GPL helper, seven requested receive channels; service start/pause/resume/finish | Passed short acceptance |
| Multiple live receivers | Concurrent radios, per-source counts and lifecycle | Pending |
| Sustained recording | Long-duration load, bounded memory and storage latency under realistic traffic | Pending |
| Other platforms/hardware | Linux radios, unplug/replug, suspend/resume, operator-assisted RF checks | Pending |

## ALFA short acceptance

The second live capture retained **406 records**, including **392 WiFi
observations**. The database transitions were recording → paused → recording →
finished. The helper reported running, paused, running again and finally stopped.
The service exited successfully. Shared history pagination returned all committed
records and preserved the ALFA source identity. A final integrity check passed.

Requested channels were 1, 6, 11, 36, 40, 44 and 48. Observations were received on
1, 6, 11, 40, 44 and 48. No reception claim is made for channel 36 from this run;
absence of traffic does not demonstrate a channel failure. RF loss is unmeasured.
Raw observations and identifying information remain in private local acceptance
artifacts, outside the repository.

The external verifier using macOS Python's SQLite library failed to reopen the
WAL database read-only after the writer exited, twice. AirHound's own shared
history reader succeeded. The acceptance result above uses that product reader;
final SQL inspection used immutable mode only after shutdown and verification
that the WAL was absent or empty. The external-reader compatibility limitation
remains to be investigated; it is not evidence that all SQLite consumers can
read a live recording without additional handling.

## Software gates

Desktop production line coverage at commit `3771115` was **85.25%**, above the
81% floor. This excludes test bodies and does not exercise physical hardware.
Later controlled-receiver tests and strict Clippy passed locally; CI and final
coverage must run against the eventual PR head before merge.
