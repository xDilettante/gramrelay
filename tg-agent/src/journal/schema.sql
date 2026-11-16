CREATE TABLE operations (
    profile TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    principal TEXT NOT NULL,
    action TEXT NOT NULL,
    payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
    state TEXT NOT NULL CHECK(state IN ('prepared','in_flight','succeeded','rejected','unknown')),
    revision INTEGER NOT NULL CHECK(revision > 0),
    worker TEXT,
    lease_until_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    expired INTEGER NOT NULL DEFAULT 0 CHECK(expired IN (0,1)),
    result_json TEXT,
    result_availability TEXT NOT NULL DEFAULT 'not_applicable' CHECK(result_availability IN ('not_applicable','stored','disabled','too_large','expired')),
    failure_json TEXT,
    PRIMARY KEY(profile,operation_id),
    CHECK((worker IS NULL) = (lease_until_ms IS NULL)),
    CHECK(state != 'in_flight' OR worker IS NOT NULL),
    CHECK(state IN ('prepared','in_flight') OR worker IS NULL),
    CHECK((result_json IS NOT NULL) = (result_availability = 'stored'))
) STRICT;
CREATE INDEX operations_recovery ON operations(state,lease_until_ms);
CREATE INDEX operations_retention ON operations(expired,expires_at_ms);
