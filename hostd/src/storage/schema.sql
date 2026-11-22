CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    applied_at_us INTEGER NOT NULL
) STRICT;
CREATE TABLE runs (
    id TEXT PRIMARY KEY NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('recording','paused','finished','failed','interrupted')),
    mode TEXT NOT NULL CHECK(mode IN ('live','replay','import')),
    started_at_us INTEGER NOT NULL CHECK(started_at_us >= 0),
    ended_at_us INTEGER,
    config_json TEXT NOT NULL CHECK(json_valid(config_json)),
    record_count INTEGER NOT NULL DEFAULT 0 CHECK(record_count >= 0)
) STRICT;
CREATE TABLE transitions (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    state TEXT NOT NULL,
    at_us INTEGER NOT NULL CHECK(at_us >= 0),
    reason TEXT NOT NULL
) STRICT;
CREATE TABLE start_requests (
    request_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
    request_json TEXT NOT NULL CHECK(json_valid(request_json))
) STRICT;
CREATE TABLE sources (
    run_id TEXT NOT NULL REFERENCES runs(id),
    source_id TEXT NOT NULL,
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    PRIMARY KEY(run_id,source_id)
) STRICT;
CREATE TABLE records (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    received_at_us INTEGER NOT NULL CHECK(received_at_us >= 0),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    FOREIGN KEY(run_id,source_id) REFERENCES sources(run_id,source_id)
) STRICT;
CREATE INDEX records_run_cursor ON records(run_id,id);
CREATE INDEX records_source_cursor ON records(run_id,source_id,id);
CREATE INDEX records_kind_time ON records(run_id,kind,received_at_us);
