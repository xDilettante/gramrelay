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
    assert_eq!(version, 2);
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
