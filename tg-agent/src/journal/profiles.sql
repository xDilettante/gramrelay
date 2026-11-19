CREATE TABLE account_profiles (
    profile TEXT NOT NULL PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('bot_api','mtproto_bot','mtproto_user')),
    account_id INTEGER NOT NULL CHECK(account_id >= 0 AND (kind = 'bot_api' OR account_id > 0)),
    UNIQUE(kind,account_id)
) STRICT;
