CREATE TABLE mtproto_session (
    id INTEGER PRIMARY KEY CHECK(id=1),
    format_version INTEGER NOT NULL CHECK(format_version=1),
    storage_id BLOB NOT NULL CHECK(typeof(storage_id)='blob' AND length(storage_id)=16),
    revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>=0),
    nonce BLOB NOT NULL CHECK(typeof(nonce)='blob' AND length(nonce)=24),
    ciphertext BLOB NOT NULL CHECK(typeof(ciphertext)='blob'),
    owner BLOB,
    lease_until_ms INTEGER,
    CHECK((owner IS NULL AND lease_until_ms IS NULL) OR (owner IS NOT NULL AND typeof(owner)='blob' AND length(owner)=16 AND typeof(lease_until_ms)='integer'))
);
