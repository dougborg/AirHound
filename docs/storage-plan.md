# Durable recording and history

Status: approved direction, implementation pending. SQLite is the proposed first
macOS/Linux backend store, not a claim of measured capacity or completed storage.
Tracking: [contracts #54](https://github.com/dougborg/AirHound/issues/54),
[lifecycle #56](https://github.com/dougborg/AirHound/issues/56),
[SQLite #57](https://github.com/dougborg/AirHound/issues/57),
[evidence #55](https://github.com/dougborg/AirHound/issues/55), and
[retention #59](https://github.com/dougborg/AirHound/issues/59).

## Priority and ownership

This is P1 immediately after local service/client integration, alongside
backend-owned Start/Pause/Resume/Finish. Build a small durable recording and query
path before extensive GUI history, mobile synchronization or broad receiver work.
All clients use the shared Rust API; only the backend owns the live database.
Existing NDJSON and WiGLE CSV outputs remain supported.

SQLite fits an offline local service with one coordinated writer and multiple
readers. PostgreSQL becomes worth considering for a central multi-user deployment
with many independent writers. Historical columnar analysis may be a complementary
export later. Neither is a prerequisite for this milestone. Validate this choice
against replay ingestion, concurrent history queries, saturation and crash
recovery before coupling frontend features to it.

## First schema

Use versioned STRICT tables, foreign keys enabled per connection, and explicit
constraints. Keep frequently queried fields typed and indexed; validated JSON
TEXT preserves extensible evidence/configuration/protocol fields. Do not duplicate
an observation only to record its detection result.

| Records | Required semantics |
| --- | --- |
| Runs and transitions | Durable run identity, configuration, policy/rule-pack versions, requested/effective state, assignment and cleanup outcomes |
| Sources | Physical receiver, gateway and source boot/restart identity; reported capabilities and loss counters |
| Observations | Source/session relation, original and receipt timestamps, clock semantics, sequence, live/replay/import provenance, identifiers, signal and protocol payload |
| Evaluations | Observation relation, rule/version, evidence/reasons and alert disposition; historical results remain intact after re-evaluation |
| Location fixes/associations | Provider, fix time, freshness, accuracy and explicit co-location relationship |
| Capture files | Stable references and metadata for larger raw packet files; missing/orphaned-file handling |

Use signed 64-bit INTEGER for database counts/timestamps with named units and
checked conversions; REAL for coordinates/accuracy; BLOB for exact binary fields
with length/format checks; TEXT for namespaced identifiers and validated JSON.
Unknown RSSI, frequency, channel or position stays NULL. MAC is optional and needs
address type/context; platform BLE identifiers are different fields. An observed
address is not a confirmed physical-device identity. Preserve full UUID meaning
and binary SSID/manufacturer/service payloads rather than comma-separated short
UUIDs or lossy text conversions.

Start with session/time/source indexes needed by bounded paginated queries.
Generated columns or expression indexes may extract selected JSON fields when a
real query requires them. JSONB, spatial R-tree indexes and analytical exports are
later measured optimizations, not first-schema requirements.

## Writer and lifecycle

Use a bounded dedicated writer with batched transactions. Blocking database calls
must not run in radio callbacks or async control handlers. Report parsed, queued,
committed, dropped and failed records separately. No silent per-device sampling;
optional aggregation/retention is an explicit policy with accounting.

Use WAL on local storage with a documented durability policy, commit boundaries,
checkpoint behavior and query limits. Remote clients connect to the service, not
a shared database file. The writer must expose disk-full/failed-commit state and
must not acknowledge records as durable merely because they entered a queue.

Pause retains the run identity and records the acquisition gap. Finish stops
receivers, drains accepted records, commits and reports export/cleanup outcomes.
Restart marks unfinished runs interrupted/recovery-required without starting
radios. Observation/evidence links must remain atomic or recoverably consistent.
Backups/exports must include a consistent committed state, not just copy an active
database file without accounting for its WAL.

## Acceptance sequence

1. Schema/contract fixtures: missing identifiers, binary data, full UUIDs, unknown
   measurements, source restart, timestamp precision, replay provenance and
   historical evidence/policy.
2. Offline replay round-trip, bounded history queries, migration and restart tests.
3. Failure/load tests: bounded memory, slow readers/writer, write errors/disk-full,
   queue overflow, commit/flush failures and shutdown under telemetry load. Measure
   sustained observations/second, commit latency, database growth and memory;
   publish workload and limits rather than inventing a universal capacity claim.
4. One known receiver baseline; then two; then mixed WiFi/BLE/GPS. Compare counts,
   source identity, timestamps, location association, reported drops and cleanup.
5. Add discovered devices one at a time. Verify capabilities before capture;
   accept channel assignments only from sources that support and acknowledge them.
   Exercise disconnect/reconnect and longer runs. Hardware-dependent checks stay
   explicit when no operator or controlled RF setup is available.
6. Add minimal history/run controls across native UI, CLI, TUI and MCP. Phone
   location/capture/controller/gateway roles follow the same backend contract.
7. Prepare a labeled release preview: verified assets, clean-install checks,
   release notes, tested hardware matrix and explicit experimental limitations.

Firmware storage is separately scoped; this does not add SQLite to no_std targets.
Discovery presence, zero software queue drops, parser tests and short captures do
not establish complete RF coverage or production readiness.

## References

- [Appropriate SQLite uses](https://www.sqlite.org/whentouse.html)
- [STRICT tables](https://www.sqlite.org/stricttables.html)
- [WAL concurrency and durability](https://www.sqlite.org/wal.html)
- [JSON functions and JSONB](https://www.sqlite.org/json1.html)
- [Generated columns](https://www.sqlite.org/gencol.html)
- [Spatial R-tree module](https://www.sqlite.org/rtree.html)

Implementation and staged validation are tracked in the
[storage acceptance record](storage-acceptance.md). It distinguishes offline
fixtures, short live checks and outstanding hardware/soak validation.
