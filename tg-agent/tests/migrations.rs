use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use tg_agent::journal::{
    Journal, JournalErrorKind, JournalOptions, OperationRequest, OperationState, Preparation,
};

fn version_one(path: &std::path::Path) -> Connection {
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    drop(file.open(path).unwrap());
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(include_str!("../src/journal/schema.sql"))
        .unwrap();
    connection
        .pragma_update(None, "application_id", 0x54474131i64)
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    connection
}

fn version_five(path: &std::path::Path) -> Connection {
    let connection = version_one(path);
    for ddl in [
        include_str!("../src/updates/schema.sql"),
        include_str!("../src/updates/consumers.sql"),
        include_str!("../src/journal/authorization.sql"),
        include_str!("../src/journal/files.sql"),
    ] {
        connection.execute_batch(ddl).unwrap();
    }
    connection.pragma_update(None, "user_version", 5).unwrap();
    connection
}

#[tokio::test]
async fn version_six_adds_mtproto_storage_without_rebinding_accounts_or_changing_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v6-updates.sqlite");
    let connection = version_five(&path);
    connection
        .execute_batch(include_str!("../src/journal/profiles.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 6).unwrap();
    connection
        .execute(
            "INSERT INTO account_profiles VALUES('personal','mtproto_user',42)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO executor_budgets VALUES('personal','alice','quota',60000,0,3,2,17,4)",
            [],
        )
        .unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row("SELECT kind,account_id FROM account_profiles", [], |r| Ok(
                (r.get::<_, String>(0)?, r.get::<_, i64>(1)?)
            ))
            .unwrap(),
        ("mtproto_user".into(), 42)
    );
    assert_eq!(
        connection
            .query_row("SELECT upload_bytes FROM executor_budgets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        17
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM mtproto_update_feeds", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn malformed_version_six_does_not_partially_create_mtproto_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad-v6.sqlite");
    let connection = version_five(&path);
    connection
        .execute_batch(include_str!("../src/journal/profiles.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 6).unwrap();
    connection
        .execute("DROP INDEX executor_files_owner", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='mtproto_update_feeds'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn current_mtproto_schema_is_verified_even_in_storage_only_builds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad-v7.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("DROP INDEX mtproto_updates_retention", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[tokio::test]
async fn malformed_version_five_cannot_partially_create_account_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad-v5.sqlite");
    let connection = version_five(&path);
    connection
        .execute("DROP INDEX executor_files_owner", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='account_profiles'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn version_six_binding_constraints_and_exact_schema_are_checked_on_reopen() {
    use tg_agent::profiles::ProfileIdentity;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v6.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    journal
        .bind_profile("user", ProfileIdentity::mtproto_user(42).unwrap())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    for sql in [
        "INSERT INTO account_profiles VALUES('alias','mtproto_user',42)",
        "INSERT INTO account_profiles VALUES('zero','mtproto_user',0)",
        "INSERT INTO account_profiles VALUES('unknown','unknown',1)",
    ] {
        assert!(connection.execute(sql, []).is_err());
    }
    connection
        .execute(
            "ALTER TABLE account_profiles ADD COLUMN unreviewed TEXT",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT account_id FROM account_profiles WHERE profile='user'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        42
    );
}

#[tokio::test]
async fn version_five_binds_only_known_feeds_and_preserves_legacy_operation_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v5-profiles.sqlite");
    let connection = version_five(&path);
    connection.execute("INSERT INTO update_feeds(profile,bot_id,retention_ms,max_event_bytes,max_batch_bytes,poll_offset,poll_revision) VALUES('known-bot',42,604800000,1048576,1048576,99,7)", []).unwrap();
    connection.execute("INSERT INTO operations(profile,operation_id,principal,action,payload_hash,state,revision,created_at_ms,updated_at_ms,expires_at_ms) VALUES('orphan','retained','alice','send_message',zeroblob(32),'unknown',3,1,2,?1)", [i64::MAX]).unwrap();
    connection
        .execute(
            "INSERT INTO executor_budgets VALUES('orphan','alice','budget',60000,0,3,2,17,4)",
            [],
        )
        .unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7);
    let binding: (String, i64) = connection
        .query_row(
            "SELECT kind,account_id FROM account_profiles WHERE profile='known-bot'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(binding, ("bot_api".into(), 42));
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM account_profiles", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "orphan keys cannot establish an account identity");
    assert_eq!(
        connection
            .query_row(
                "SELECT poll_offset,poll_revision FROM update_feeds",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (99, 7)
    );
    assert_eq!(
        connection
            .query_row("SELECT upload_bytes FROM executor_budgets", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        17
    );
    drop(connection);
    let retained = journal
        .get("orphan", "alice", "retained")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (retained.state(), retained.revision()),
        (OperationState::Unknown, 3)
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn migration_preserves_receipts_hashes_versions_and_another_workers_flight() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.sqlite");
    let connection = version_one(&path);
    let payload = json!({"text":"PRIVATE_ORIGINAL"});
    let hash = Sha256::digest(serde_json::to_vec(&payload).unwrap());
    connection.execute("INSERT INTO operations(profile,operation_id,principal,action,payload_hash,state,revision,created_at_ms,updated_at_ms,expires_at_ms,result_json,result_availability) VALUES('bot','done','alice','send_message',?1,'succeeded',7,1,2,?2,'{\"message_id\":42}','stored')",params![hash.as_slice(),i64::MAX]).unwrap();
    connection.execute("INSERT INTO operations(profile,operation_id,principal,action,payload_hash,state,revision,worker,lease_until_ms,created_at_ms,updated_at_ms,expires_at_ms) VALUES('bot','active','alice','send_message',?1,'in_flight',3,'other-worker',?2,1,2,?2)",params![hash.as_slice(),i64::MAX]).unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let version: i64 = Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7);
    let request = OperationRequest::new("bot", "alice", "done", "send_message", &payload).unwrap();
    assert!(
        matches!(journal.prepare(request).await.unwrap(),Preparation::Existing(record) if record.state() == OperationState::Succeeded && record.revision() == 7 && record.result().unwrap()["message_id"] == 42)
    );
    let active = journal
        .get("bot", "alice", "active")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (active.state(), active.revision()),
        (OperationState::InFlight, 3)
    );
    journal.close().await.unwrap();
    Journal::open(&path, JournalOptions::default())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unreviewed_old_schema_is_not_partially_migrated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.sqlite");
    let connection = version_one(&path);
    connection
        .execute("DROP INDEX operations_retention", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='update_feeds'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn version_two_migrates_without_rewinding_update_or_poll_positions() {
    use tg_agent::updates::{SourceMode, UpdateFeed, UpdateOptions};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v2.sqlite");
    let connection = version_one(&path);
    connection
        .execute_batch(include_str!("../src/updates/schema.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    let payload=serde_json::to_vec(&json!({"update_id":10,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"retained"}})).unwrap();
    let hash = Sha256::digest(&payload);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    connection.execute("INSERT INTO update_feeds(profile,bot_id,retention_ms,max_event_bytes,max_batch_bytes,last_sequence,last_received_at_ms,entry_count,byte_count,source_mode,source_revision,poll_offset,poll_revision,poll_updated_at_ms) VALUES('bot',1,604800000,1048576,1048576,1,?1,1,?2,'polling',2,11,8,?1)",params![time,payload.len() as i64]).unwrap();
    connection.execute("INSERT INTO updates(profile,sequence,update_id,payload,payload_hash,chat_id,received_at_ms,expires_at_ms) VALUES('bot',1,10,?1,?2,1,?3,?4)",params![payload,hash.as_slice(),time,i64::MAX]).unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let lease = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(
        feed.poll_checkpoint(&lease).await.unwrap().offset(),
        Some(11)
    );
    assert_eq!(
        feed.read(None, 100).await.unwrap().events()[0]
            .update()
            .update_id,
        10
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn version_three_keeps_active_consumer_ownership_and_validates_the_new_unique_index() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v3.sqlite");
    let connection = version_one(&path);
    connection
        .execute_batch(include_str!("../src/updates/schema.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../src/updates/consumers.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
    connection.execute("INSERT INTO update_feeds(profile,bot_id,retention_ms,max_event_bytes,max_batch_bytes) VALUES('bot',123,604800000,1048576,1048576)",[]).unwrap();
    connection.execute("INSERT INTO update_consumers(profile,consumer_id,position,worker,revision,lease_until_ms) VALUES('bot','retained',0,'other-worker',7,?1)",[i64::MAX]).unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let retained: (String, i64, i64) = connection
        .query_row(
            "SELECT worker,revision,lease_until_ms FROM update_consumers",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retained, ("other-worker".into(), 7, i64::MAX));
    connection
        .execute("DROP INDEX executor_callback_admission", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
}

#[tokio::test]
async fn malformed_version_three_is_not_partially_upgraded_to_executor_storage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid-v3.sqlite");
    let connection = version_one(&path);
    connection
        .execute_batch(include_str!("../src/updates/schema.sql"))
        .unwrap();
    connection
        .execute_batch(include_str!("../src/updates/consumers.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
    connection
        .execute("DROP INDEX update_consumers_owner", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='executor_budgets'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn version_four_preserves_durable_usage_and_creates_validated_file_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v4.sqlite");
    let connection = version_one(&path);
    for ddl in [
        include_str!("../src/updates/schema.sql"),
        include_str!("../src/updates/consumers.sql"),
        include_str!("../src/journal/authorization.sql"),
    ] {
        connection.execute_batch(ddl).unwrap();
    }
    connection.pragma_update(None, "user_version", 4).unwrap();
    connection
        .execute(
            "INSERT INTO executor_budgets VALUES('bot','owner','budget',60000,0,3,2,17,4)",
            [],
        )
        .unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row("SELECT upload_bytes FROM executor_budgets", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        17
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM executor_files", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    connection
        .execute("DROP INDEX executor_files_owner", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
}

#[tokio::test]
async fn malformed_version_four_does_not_partially_create_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad-v4.sqlite");
    let connection = version_one(&path);
    for ddl in [
        include_str!("../src/updates/schema.sql"),
        include_str!("../src/updates/consumers.sql"),
        include_str!("../src/journal/authorization.sql"),
    ] {
        connection.execute_batch(ddl).unwrap();
    }
    connection.pragma_update(None, "user_version", 4).unwrap();
    connection
        .execute("DROP INDEX executor_callback_admission", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::UnsupportedSchema
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='executor_files'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
