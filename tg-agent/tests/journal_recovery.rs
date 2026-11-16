use rusqlite::{Connection, params};
use serde_json::json;
use tg_agent::journal::{
    Completion, Journal, JournalErrorKind, JournalOptions, OperationRequest, OperationState,
    Preparation, Prepared, ResultAvailability,
};

fn request(id: &str) -> OperationRequest {
    OperationRequest::new(
        "bot",
        "alice",
        id,
        "send_message",
        &json!({"chat_id":1,"text":"PRIVATE_REQUEST"}),
    )
    .unwrap()
}
fn ready(value: Preparation) -> Prepared {
    match value {
        Preparation::Ready(value) => value,
        other => panic!("not executable: {other:?}"),
    }
}
fn expire_lease(path: &std::path::Path, id: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE operations SET lease_until_ms=0 WHERE operation_id=?1",
            params![id],
        )
        .unwrap();
}
fn expire_lifetime(path: &std::path::Path, id: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE operations SET expires_at_ms=0 WHERE operation_id=?1",
            params![id],
        )
        .unwrap();
}

#[tokio::test]
async fn recovery_fences_expired_attempts_and_preserves_another_active_worker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let stale = first
        .begin(ready(first.prepare(request("stale")).await.unwrap()))
        .await
        .unwrap();
    let active = first
        .begin(ready(first.prepare(request("active")).await.unwrap()))
        .await
        .unwrap();
    expire_lease(&path, "stale");
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    assert_eq!(second.recover_expired(10).await.unwrap(), 1);
    assert_eq!(second.recover_expired(10).await.unwrap(), 0);
    assert_eq!(
        second
            .get("bot", "alice", "stale")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Unknown
    );
    assert_eq!(
        second
            .get("bot", "alice", "active")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::InFlight
    );
    assert!(
        matches!(second.prepare(request("stale")).await.unwrap(),Preparation::Existing(operation) if operation.state() == OperationState::Unknown)
    );
    assert_eq!(
        first
            .complete(stale, Completion::succeeded(json!({"message_id":1})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    first
        .complete(active, Completion::succeeded(json!({"message_id":2})))
        .await
        .unwrap();
}

#[tokio::test]
async fn per_key_recovery_never_reissues_a_flight_but_can_reclaim_unstarted_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let stale = ready(first.prepare(request("prepared")).await.unwrap());
    expire_lease(&path, "prepared");
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let reclaimed = ready(second.prepare(request("prepared")).await.unwrap());
    assert_eq!(
        first.begin(stale).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    let flight = second.begin(reclaimed).await.unwrap();
    expire_lease(&path, "prepared");
    assert!(
        matches!(first.prepare(request("prepared")).await.unwrap(),Preparation::Existing(operation) if operation.state() == OperationState::Unknown)
    );
    assert_eq!(
        second
            .complete(flight, Completion::succeeded(json!({})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
}

#[tokio::test]
async fn ttl_removes_receipts_but_keeps_keys_and_expired_preparations_cannot_send() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_operations(2))
        .await
        .unwrap();
    let flight = journal
        .begin(ready(journal.prepare(request("sent")).await.unwrap()))
        .await
        .unwrap();
    journal
        .complete(flight, Completion::succeeded(json!({"message_id":42})))
        .await
        .unwrap();
    let prepared = ready(journal.prepare(request("unstarted")).await.unwrap());
    expire_lifetime(&path, "sent");
    expire_lifetime(&path, "unstarted");
    assert_eq!(journal.prune_expired(1).await.unwrap(), 1);
    assert_eq!(journal.prune_expired(1).await.unwrap(), 1);
    assert_eq!(journal.prune_expired(1).await.unwrap(), 0);
    let operation = journal.get("bot", "alice", "sent").await.unwrap().unwrap();
    assert_eq!(operation.state(), OperationState::Succeeded);
    assert_eq!(operation.result_availability(), ResultAvailability::Expired);
    assert!(operation.expired() && operation.result().is_none());
    assert!(
        matches!(journal.prepare(request("sent")).await.unwrap(),Preparation::Existing(operation) if operation.expired())
    );
    assert!(
        matches!(journal.prepare(request("unstarted")).await.unwrap(),Preparation::Existing(operation) if operation.expired())
    );
    assert_eq!(
        journal.begin(prepared).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        journal.prepare(request("new")).await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    assert!(journal.recover_expired(0).await.is_err());
    assert!(journal.prune_expired(1001).await.is_err());
}

#[tokio::test]
async fn reading_one_expired_key_prunes_it_without_disclosing_another_principals_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let flight = journal
        .begin(ready(journal.prepare(request("one")).await.unwrap()))
        .await
        .unwrap();
    journal
        .complete(
            flight,
            Completion::succeeded(json!({"PRIVATE_RESULT":"test"})),
        )
        .await
        .unwrap();
    expire_lifetime(&path, "one");
    assert!(
        journal
            .get("bot", "mallory", "one")
            .await
            .unwrap()
            .is_none()
    );
    let operation = journal.get("bot", "alice", "one").await.unwrap().unwrap();
    assert!(operation.expired() && operation.result().is_none());
    let stored: Option<String> = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT result_json FROM operations WHERE operation_id='one'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(stored.is_none());
}

#[tokio::test]
async fn missing_or_oversized_cache_never_erases_known_success() {
    let directory = tempfile::tempdir().unwrap();
    for (name, options, availability) in [
        (
            "disabled",
            JournalOptions::default().retain_results(false),
            ResultAvailability::Disabled,
        ),
        (
            "oversized",
            JournalOptions::default().max_result_bytes(8),
            ResultAvailability::TooLarge,
        ),
    ] {
        let journal = Journal::open(directory.path().join(format!("{name}.sqlite")), options)
            .await
            .unwrap();
        let flight = journal
            .begin(ready(journal.prepare(request("one")).await.unwrap()))
            .await
            .unwrap();
        let operation = journal
            .complete(flight, Completion::succeeded(json!({"message_id":42})))
            .await
            .unwrap();
        assert_eq!(operation.state(), OperationState::Succeeded);
        assert_eq!(operation.result_availability(), availability);
        assert!(operation.result().is_none());
        assert!(matches!(
            journal.prepare(request("one")).await.unwrap(),
            Preparation::Existing(_)
        ));
    }
}

#[tokio::test]
async fn known_not_sent_can_be_retried_while_refused_and_unknown_remain_terminal() {
    use tg::test_kit::{MockResponse, MockTransport, Step};
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("operations.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([Step::reply(
        "sendMessage",
        MockResponse::json(json!({"ok":false,"error_code":403,"description":"PRIVATE_REMOTE"}))
            .status(403),
    )])
    .unwrap();
    let bot = mock.bot().unwrap();
    let local = bot.send_message(1.into(), "").await.unwrap_err();
    let flight = journal
        .begin(ready(journal.prepare(request("local")).await.unwrap()))
        .await
        .unwrap();
    let stale = flight.clone();
    assert_eq!(
        journal
            .complete(flight, Completion::from_error(&local))
            .await
            .unwrap()
            .state(),
        OperationState::Prepared
    );
    let retry = ready(journal.prepare(request("local")).await.unwrap());
    assert_eq!(
        journal
            .complete(stale, Completion::succeeded(json!({})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let flight = journal.begin(retry).await.unwrap();
    journal
        .complete(flight, Completion::succeeded(json!({})))
        .await
        .unwrap();
    let refused = bot.send_message(1.into(), "test").await.unwrap_err();
    for (id, error, state) in [
        ("refused", refused, OperationState::Rejected),
        (
            "unknown",
            tg::Error::handler("PRIVATE_HANDLER"),
            OperationState::Unknown,
        ),
    ] {
        let flight = journal
            .begin(ready(journal.prepare(request(id)).await.unwrap()))
            .await
            .unwrap();
        let operation = journal
            .complete(flight, Completion::from_error(&error))
            .await
            .unwrap();
        assert_eq!(operation.state(), state);
        assert!(
            !serde_json::to_string(&operation)
                .unwrap()
                .contains("PRIVATE_")
        );
        assert!(matches!(
            journal.prepare(request(id)).await.unwrap(),
            Preparation::Existing(_)
        ));
    }
}

#[tokio::test]
async fn an_unchanged_late_confirmation_can_finish_but_another_worker_cannot_use_its_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let lease = ready(first.prepare(request("one")).await.unwrap());
    assert_eq!(
        second.begin(lease.clone()).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    let flight = first.begin(lease).await.unwrap();
    assert_eq!(
        second
            .complete(flight.clone(), Completion::succeeded(json!({})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    expire_lease(&path, "one");
    assert_eq!(
        first
            .complete(flight, Completion::succeeded(json!({"message_id":42})))
            .await
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
}

#[tokio::test]
async fn foreign_newer_incomplete_and_corrupt_databases_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    for mode in ["foreign", "newer", "incomplete", "corrupt"] {
        let path = directory.path().join(format!("{mode}.sqlite"));
        let mut create = std::fs::OpenOptions::new();
        create.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600);
        }
        drop(create.open(&path).unwrap());
        if mode == "corrupt" {
            std::fs::write(&path, b"PRIVATE_CORRUPT_DATABASE").unwrap();
        } else {
            let connection = Connection::open(&path).unwrap();
            match mode {
                "foreign" => {
                    connection
                        .execute("CREATE TABLE private_data(value TEXT)", [])
                        .unwrap();
                }
                "newer" => {
                    connection
                        .pragma_update(None, "application_id", 0x54474131i64)
                        .unwrap();
                    connection.pragma_update(None, "user_version", 99).unwrap();
                }
                _ => {
                    connection
                        .pragma_update(None, "application_id", 0x54474131i64)
                        .unwrap();
                    connection.pragma_update(None, "user_version", 1).unwrap();
                }
            }
        }
        let error = Journal::open(&path, JournalOptions::default())
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("PRIVATE_"));
        if mode != "corrupt" {
            assert_eq!(error.kind(), JournalErrorKind::UnsupportedSchema);
            let mode: String = Connection::open(&path)
                .unwrap()
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .unwrap();
            assert_eq!(mode, "delete");
        }
    }
    for options in [
        JournalOptions::default().max_pending(0),
        JournalOptions::default().retention(std::time::Duration::MAX),
        JournalOptions::default().max_result_bytes(usize::MAX),
    ] {
        assert!(
            Journal::open(directory.path().join("invalid.sqlite"), options)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn concurrent_startup_on_a_new_database_keeps_one_complete_schema() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..5 {
        let path = directory.path().join(format!("start-{index}.sqlite"));
        let (first, second) = tokio::join!(
            Journal::open(&path, JournalOptions::default()),
            Journal::open(&path, JournalOptions::default())
        );
        let first = first.unwrap();
        let second = second.unwrap();
        let (a, b) = tokio::join!(
            first.prepare(request("one")),
            second.prepare(request("one"))
        );
        assert_eq!(
            [a.unwrap(), b.unwrap()]
                .iter()
                .filter(|value| matches!(value, Preparation::Ready(_)))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn cached_refusal_preserves_safe_server_advice_without_retargeting_or_descriptions() {
    use tg::test_kit::{MockResponse, MockTransport, Step};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let mock = MockTransport::new([Step::reply("sendMessage",MockResponse::json(json!({"ok":false,"error_code":429,"description":"PRIVATE_ERROR_DESCRIPTION","parameters":{"retry_after":7,"migrate_to_chat_id":-1009}})).status(429))]).unwrap();
    let error = mock
        .bot()
        .unwrap()
        .send_message(1.into(), "test")
        .await
        .unwrap_err();
    let flight = journal
        .begin(ready(journal.prepare(request("one")).await.unwrap()))
        .await
        .unwrap();
    journal
        .complete(flight, Completion::from_error(&error))
        .await
        .unwrap();
    drop(journal);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let operation = journal.get("bot", "alice", "one").await.unwrap().unwrap();
    let failure = operation.failure().unwrap();
    assert_eq!(
        failure.retry_after(),
        Some(std::time::Duration::from_secs(7))
    );
    assert_eq!(failure.migrate_to_chat_id(), Some((-1009).into()));
    assert!(
        !serde_json::to_string(&operation)
            .unwrap()
            .contains("PRIVATE_")
    );
    assert!(matches!(
        journal.prepare(request("one")).await.unwrap(),
        Preparation::Existing(_)
    ));
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn explicit_close_affects_all_clones_and_releases_only_unstarted_reservations() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let unstarted = ready(journal.prepare(request("unstarted")).await.unwrap());
    let _flight = journal
        .begin(ready(journal.prepare(request("active")).await.unwrap()))
        .await
        .unwrap();
    let clone = journal.clone();
    let other = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let other_lease = ready(other.prepare(request("other-worker")).await.unwrap());
    journal.close().await.unwrap();
    clone.close().await.unwrap();
    assert_eq!(
        clone
            .get("bot", "alice", "unstarted")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Closed
    );
    assert_eq!(
        clone.begin(unstarted.clone()).await.unwrap_err().kind(),
        JournalErrorKind::Closed
    );
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let next = ready(reopened.prepare(request("unstarted")).await.unwrap());
    assert!(
        matches!(reopened.prepare(request("other-worker")).await.unwrap(),Preparation::Existing(operation) if operation.state() == OperationState::Prepared)
    );
    let other_flight = other.begin(other_lease).await.unwrap();
    other
        .complete(other_flight, Completion::succeeded(json!({})))
        .await
        .unwrap();
    other.close().await.unwrap();
    assert_eq!(
        reopened.begin(unstarted).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    assert!(
        matches!(reopened.prepare(request("active")).await.unwrap(),Preparation::Existing(operation) if operation.state() == OperationState::InFlight)
    );
    let flight = reopened.begin(next).await.unwrap();
    reopened
        .complete(flight, Completion::succeeded(json!({})))
        .await
        .unwrap();
    reopened.close().await.unwrap();
    assert!(
        Journal::open(
            directory.path().join("invalid.sqlite"),
            JournalOptions::default().max_pending(1025)
        )
        .await
        .is_err()
    );
}
