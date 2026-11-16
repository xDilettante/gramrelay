CREATE TABLE executor_budgets (
    profile TEXT NOT NULL,
    principal TEXT NOT NULL,
    budget_id TEXT NOT NULL,
    window_ms INTEGER NOT NULL CHECK(window_ms > 0),
    window_start_ms INTEGER NOT NULL CHECK(window_start_ms >= 0),
    invocations INTEGER NOT NULL CHECK(invocations >= 0),
    write_attempts INTEGER NOT NULL CHECK(write_attempts >= 0),
    upload_bytes INTEGER NOT NULL CHECK(upload_bytes >= 0),
    cost_units INTEGER NOT NULL CHECK(cost_units >= 0),
    PRIMARY KEY(profile,principal,budget_id)
) STRICT;
CREATE TABLE executor_operation_scopes (
    profile TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    bot_id INTEGER NOT NULL CHECK(bot_id >= 0),
    action TEXT NOT NULL CHECK(action IN ('send_message','edit_message_text','answer_callback_query','send_document')),
    chat_id INTEGER CHECK(chat_id != 0),
    file_handle TEXT,
    file_sha256 BLOB CHECK(length(file_sha256) = 32),
    file_bytes INTEGER CHECK(file_bytes >= 0),
    reference_hash BLOB CHECK(length(reference_hash) = 32),
    admitted INTEGER NOT NULL DEFAULT 0 CHECK(admitted IN (0,1)),
    PRIMARY KEY(profile,operation_id),
    FOREIGN KEY(profile,operation_id) REFERENCES operations(profile,operation_id),
    CHECK((action = 'send_document') = (file_handle IS NOT NULL)),
    CHECK((file_handle IS NULL) = (file_sha256 IS NULL)),
    CHECK((file_handle IS NULL) = (file_bytes IS NULL)),
    CHECK((action = 'answer_callback_query') = (reference_hash IS NOT NULL)),
    CHECK(action = 'answer_callback_query' OR chat_id IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX executor_callback_admission ON executor_operation_scopes(profile,reference_hash) WHERE admitted = 1 AND reference_hash IS NOT NULL;
