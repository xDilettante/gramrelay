CREATE TABLE update_feeds (
    profile TEXT PRIMARY KEY NOT NULL,
    bot_id INTEGER UNIQUE NOT NULL CHECK(bot_id >= 0),
    retention_ms INTEGER NOT NULL CHECK(retention_ms > 0),
    max_event_bytes INTEGER NOT NULL CHECK(max_event_bytes > 0),
    max_batch_bytes INTEGER NOT NULL CHECK(max_batch_bytes >= max_event_bytes),
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_sequence >= 0),
    floor_sequence INTEGER NOT NULL DEFAULT 0 CHECK(floor_sequence >= 0 AND floor_sequence <= last_sequence),
    last_received_at_ms INTEGER NOT NULL DEFAULT 0,
    entry_count INTEGER NOT NULL DEFAULT 0 CHECK(entry_count >= 0),
    byte_count INTEGER NOT NULL DEFAULT 0 CHECK(byte_count >= 0),
    source_mode TEXT CHECK(source_mode IN ('polling','webhook')),
    source_worker TEXT,
    source_revision INTEGER NOT NULL DEFAULT 0 CHECK(source_revision >= 0),
    source_until_ms INTEGER,
    poll_offset INTEGER CHECK(poll_offset >= 0),
    poll_revision INTEGER NOT NULL DEFAULT 0 CHECK(poll_revision >= 0),
    poll_updated_at_ms INTEGER,
    CHECK((source_worker IS NULL) = (source_until_ms IS NULL)),
    CHECK(source_worker IS NULL OR source_mode IS NOT NULL)
) STRICT;
CREATE TABLE updates (
    profile TEXT NOT NULL REFERENCES update_feeds(profile),
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    update_id INTEGER NOT NULL CHECK(update_id >= 0),
    payload BLOB NOT NULL,
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    chat_id INTEGER,
    received_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    PRIMARY KEY(profile,sequence),
    UNIQUE(profile,update_id)
) STRICT;
CREATE INDEX updates_retention ON updates(expires_at_ms,profile,sequence);
