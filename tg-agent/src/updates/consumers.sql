CREATE TABLE update_consumers (
    profile TEXT NOT NULL REFERENCES update_feeds(profile),
    consumer_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK(position >= 0),
    worker TEXT,
    revision INTEGER NOT NULL CHECK(revision > 0),
    lease_until_ms INTEGER,
    PRIMARY KEY(profile,consumer_id),
    CHECK((worker IS NULL) = (lease_until_ms IS NULL))
) STRICT;
CREATE INDEX update_consumers_owner ON update_consumers(worker,lease_until_ms);
