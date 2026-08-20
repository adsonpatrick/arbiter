CREATE TABLE governor_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    occurred_at_unix_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    request_id TEXT,
    attempt_id TEXT,
    schema_version INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    attempt_index INTEGER
);

CREATE INDEX governor_events_attempt_id_idx
    ON governor_events (attempt_id);
