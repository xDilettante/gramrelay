CREATE TABLE "executor_files" (
    handle TEXT NOT NULL PRIMARY KEY,
    profile TEXT NOT NULL,
    principal TEXT NOT NULL,
    sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
    byte_size INTEGER NOT NULL CHECK(byte_size >= 0),
    filename TEXT NOT NULL CHECK(length(filename) > 0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms > created_at_ms),
    FOREIGN KEY(profile) REFERENCES account_profiles(profile)
) STRICT;
CREATE INDEX executor_files_owner ON executor_files(profile,principal,handle);
