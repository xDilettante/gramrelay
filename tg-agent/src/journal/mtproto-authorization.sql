CREATE UNIQUE INDEX account_profiles_exact ON account_profiles(profile,kind,account_id);
CREATE TABLE mtproto_operation_scopes (
    profile TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    account_kind TEXT NOT NULL CHECK(account_kind IN ('mtproto_bot','mtproto_user')),
    account_id INTEGER NOT NULL CHECK(account_id > 0),
    action TEXT NOT NULL CHECK(action IN ('mtproto_send_message','mtproto_edit_message_text','mtproto_answer_callback_query','mtproto_send_document')),
    peer_kind TEXT CHECK(peer_kind IN ('user','chat','channel')),
    peer_id INTEGER CHECK(peer_id > 0),
    file_handle TEXT,
    file_sha256 BLOB CHECK(length(file_sha256) = 32),
    file_bytes INTEGER CHECK(file_bytes >= 0),
    reference_hash BLOB CHECK(length(reference_hash) = 32),
    admitted INTEGER NOT NULL DEFAULT 0 CHECK(admitted IN (0,1)),
    PRIMARY KEY(profile,operation_id),
    FOREIGN KEY(profile,operation_id) REFERENCES operations(profile,operation_id),
    FOREIGN KEY(profile,account_kind,account_id) REFERENCES account_profiles(profile,kind,account_id),
    CHECK((peer_kind IS NULL) = (peer_id IS NULL)),
    CHECK((action = 'mtproto_send_document') = (file_handle IS NOT NULL)),
    CHECK((file_handle IS NULL) = (file_sha256 IS NULL)),
    CHECK((file_handle IS NULL) = (file_bytes IS NULL)),
    CHECK((action = 'mtproto_answer_callback_query') = (reference_hash IS NOT NULL)),
    CHECK(action = 'mtproto_answer_callback_query' OR peer_id IS NOT NULL),
    CHECK(action != 'mtproto_answer_callback_query' OR account_kind = 'mtproto_bot')
) STRICT;
CREATE UNIQUE INDEX mtproto_callback_admission ON mtproto_operation_scopes(profile,reference_hash) WHERE admitted = 1 AND reference_hash IS NOT NULL;
