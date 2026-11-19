#![cfg(feature = "executor")]
use serde_json::json;
use std::time::Duration;
use tg::{
    EffectOutcome,
    test_kit::{MockResponse, MockTransport, Step},
};
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorErrorKind, ExecutorOptions, Grant,
        PolicyStore, Principal, Request,
    },
    journal::{Journal, JournalOptions, OperationState},
    updates::{UpdateFeed, UpdateOptions},
};
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn policy() -> PolicyStore {
    let value = PolicyStore::default();
    value
        .replace(
            &actor(),
            "primary",
            Grant::new(0, Budget::new("retained-quota").unwrap())
                .allow(Action::SendMessage)
                .chats(ChatScope::only([ChatId::new(-1001).unwrap()])),
        )
        .unwrap();
    value
}
fn request(id: &str, text: &str) -> Request {
    Request::from_json(&serde_json::to_vec(&json!({"schema_version":1,"profile":"primary","operation_id":id,"call":{"tool":"send_message","arguments":{"chat_id":"-1001","text":text}}})).unwrap()).unwrap()
}
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"PRIVATE_BOT"}}),
        ),
    )
}
fn sent(chat: i64) -> Step {
    Step::reply(
        "sendMessage",
        MockResponse::json(
            json!({"ok":true,"result":{"message_id":8,"date":1,"chat":{"id":chat,"type":"supergroup","title":"PRIVATE_CHAT"},"text":"PRIVATE_TEXT","future_private":"PRIVATE_FUTURE"}}),
        ),
    )
}
fn options() -> ExecutorOptions {
    ExecutorOptions::default()
        .request_budget(Duration::from_millis(500))
        .receipt_margin(Duration::from_millis(100))
}

async fn wait_for_send(mock: &MockTransport) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if mock
                .requests()
                .unwrap()
                .iter()
                .any(|v| v.method() == "sendMessage")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_operations_share_a_write_budget_and_cached_calls_charge_only_invocations() {
    use tg_agent::executor::BudgetLimits;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let grants = policy();
    grants
        .replace(
            &actor(),
            "primary",
            Grant::new(
                0,
                Budget::new("retained-quota").unwrap().limits(BudgetLimits {
                    write_attempts: 1,
                    ..BudgetLimits::default()
                }),
            )
            .allow(Action::SendMessage)
            .chats(ChatScope::All),
        )
        .unwrap();
    let mock = MockTransport::new([identity(), sent(-1001)]).unwrap();
    let run = executor(journal, grants, &mock).await;
    let first = request("first", "hello");
    let second = request("second", "hello");
    let principal = actor();
    let (a, b) = tokio::join!(
        run.execute(&principal, &first, None),
        run.execute(&principal, &second, None)
    );
    let (winner, loser) = match (a, b) {
        (Ok(_), Err(error)) => (&first, error),
        (Err(error), Ok(_)) => (&second, error),
        other => panic!("expected one write: {other:?}"),
    };
    assert_eq!(loser.kind(), ExecutorErrorKind::QuotaExceeded);
    assert_eq!(
        run.execute(&actor(), winner, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    let usage: (i64, i64, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT invocations,write_attempts,cost_units FROM executor_budgets",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage, (3, 1, 1));
    assert_eq!(mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn revoked_during_rpc_records_success_but_denies_disclosure_until_current_access_returns() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(),Step::reply("sendMessage",MockResponse::json(json!({"ok":true,"result":{"message_id":8,"date":1,"chat":{"id":-1001,"type":"supergroup"}}})).delay(Duration::from_millis(300)))]).unwrap();
    let grants = policy();
    let run = executor(journal.clone(), grants.clone(), &mock).await;
    let worker = run.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(&actor(), &request("stable", "hello"), None)
            .await
    });
    wait_for_send(&mock).await;
    grants.revoke(&actor(), "primary").unwrap();
    let denied = task.await.unwrap().unwrap_err();
    assert_eq!(denied.kind(), ExecutorErrorKind::Denied);
    assert_eq!(denied.outcome(), Some(EffectOutcome::Unknown));
    assert_eq!(
        journal
            .get("primary", "alice", "stable")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    grants
        .replace(
            &actor(),
            "primary",
            Grant::new(0, Budget::new("retained-quota").unwrap())
                .allow(Action::SendMessage)
                .chats(ChatScope::All),
        )
        .unwrap();
    assert_eq!(
        run.execute(&actor(), &request("stable", "hello"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn wrong_identity_releases_only_the_unstarted_reservation_for_the_same_key() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(
                json!({"ok":true,"result":{"id":99,"is_bot":true,"first_name":"Offline"}}),
            ),
        ),
        identity(),
        sent(-1001),
    ])
    .unwrap();
    let run = executor(journal.clone(), policy(), &mock).await;
    let call = request("same-key", "hello");
    let error = run.execute(&actor(), &call, None).await.unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::Identity);
    assert_eq!(error.outcome(), Some(EffectOutcome::NotSent));
    assert_eq!(
        journal
            .get("primary", "alice", "same-key")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Prepared
    );
    assert_eq!(
        run.execute(&actor(), &call, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(mock.requests().unwrap().len(), 3);
}

#[tokio::test]
async fn cancellation_after_begin_recovers_unknown_and_never_reissues_the_request() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default().lease_duration(Duration::from_millis(250)),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(), Step::timeout("sendMessage")]).unwrap();
    let run = Executor::new(
        policy(),
        ExecutorOptions::default()
            .request_budget(Duration::from_millis(100))
            .receipt_margin(Duration::from_millis(50)),
    )
    .unwrap();
    run.register_profile(
        UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
            .await
            .unwrap(),
        mock.bot().unwrap(),
    )
    .unwrap();
    let worker = run.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(&actor(), &request("cancelled", "hello"), None)
            .await
    });
    wait_for_send(&mock).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let call = request("cancelled", "hello");
    assert_eq!(
        run.execute(&actor(), &call, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::InFlight
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    journal.recover_expired(10).await.unwrap();
    assert_eq!(
        run.execute(&actor(), &call, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Unknown
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn receipt_wait_timeout_is_unknown_even_when_the_native_commit_can_finish_later() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let journal = Journal::open(
        &path,
        JournalOptions::default().lease_duration(Duration::from_millis(300)),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(),Step::reply("sendMessage",MockResponse::json(json!({"ok":true,"result":{"message_id":8,"date":1,"chat":{"id":-1001,"type":"supergroup"}}})).delay(Duration::from_millis(60)))]).unwrap();
    let run = Executor::new(
        policy(),
        ExecutorOptions::default()
            .request_budget(Duration::from_millis(150))
            .receipt_margin(Duration::from_millis(50)),
    )
    .unwrap();
    run.register_profile(
        UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
            .await
            .unwrap(),
        mock.bot().unwrap(),
    )
    .unwrap();
    let worker = run.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(&actor(), &request("late-commit", "hello"), None)
            .await
    });
    wait_for_send(&mock).await;
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::Uncertain);
    assert_eq!(error.outcome(), Some(EffectOutcome::Unknown));
    blocker.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        run.execute(&actor(), &request("late-commit", "hello"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn an_existing_operation_read_failure_is_unknown_even_without_a_new_rpc() {
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };
    struct Wakeup(tokio::sync::Notify);
    impl Wake for Wakeup {
        fn wake(self: Arc<Self>) {
            self.0.notify_one();
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let mock = MockTransport::new([identity(), sent(-1001)]).unwrap();
    let run = executor(journal.clone(), policy(), &mock).await;
    let principal = actor();
    let call = request("existing", "hello");
    run.execute(&principal, &call, None).await.unwrap();
    let wake = Arc::new(Wakeup(tokio::sync::Notify::new()));
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(run.execute(&principal, &call, None));
    let blocker = rusqlite::Connection::open(&path).unwrap();
    // Each lock makes precisely the next database await pending, regardless of
    // thread scheduling. The first response completes invocation admission.
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    blocker.execute_batch("ROLLBACK").unwrap();
    tokio::time::timeout(Duration::from_secs(2), wake.0.notified())
        .await
        .unwrap();
    // The second response is prepare -> Existing; do not resume cached() yet.
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    blocker.execute_batch("ROLLBACK").unwrap();
    tokio::time::timeout(Duration::from_secs(2), wake.0.notified())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let error = future.await.unwrap_err();
    assert_eq!(error.outcome(), Some(EffectOutcome::Unknown));
    assert_eq!(mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn failure_projection_keeps_large_migration_ids_as_strings_and_diagnostics_redacted() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(),Step::reply("sendMessage",MockResponse::json(json!({"ok":false,"error_code":400,"description":"PRIVATE_REMOTE","parameters":{"migrate_to_chat_id":-9007199254740993i64}})))]).unwrap();
    let receipt = executor(journal, policy(), &mock)
        .await
        .execute(&actor(), &request("migration", "hello"), None)
        .await
        .unwrap();
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        value["operation"]["failure"]["migrate_to_chat_id"],
        "-9007199254740993"
    );
    assert!(!format!("{receipt:?}").contains("9007199254740993"));
    assert!(!value.to_string().contains("PRIVATE_REMOTE"));
}

#[tokio::test]
async fn revocation_while_waiting_for_sqlite_does_not_charge_or_start_identity() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let mock = MockTransport::new([]).unwrap();
    let grants = policy();
    let run = executor(journal, grants.clone(), &mock).await;
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let principal = actor();
    let call = request("revoked", "hello");
    let mut future = std::pin::pin!(run.execute(&principal, &call, None));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    grants.revoke(&principal, "primary").unwrap();
    blocker.execute_batch("ROLLBACK").unwrap();
    assert_eq!(future.await.unwrap_err().kind(), ExecutorErrorKind::Denied);
    assert!(mock.requests().unwrap().is_empty());
    let count: i64 = blocker
        .query_row("SELECT COUNT(*) FROM executor_budgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}
async fn executor(journal: Journal, policy: PolicyStore, mock: &MockTransport) -> Executor {
    let value = Executor::new(policy, options()).unwrap();
    value
        .register_profile(
            UpdateFeed::open(journal, "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            mock.bot().unwrap(),
        )
        .unwrap();
    value
}
#[tokio::test]
async fn denied_requests_are_local_and_success_cache_uses_current_scope_and_minimal_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(), sent(-1001)]).unwrap();
    let grants = PolicyStore::default();
    let run = executor(journal.clone(), grants.clone(), &mock).await;
    let call = request("stable", "PRIVATE_TEXT");
    assert_eq!(
        run.execute(&actor(), &call, None).await.unwrap_err().kind(),
        ExecutorErrorKind::Denied
    );
    assert!(mock.requests().unwrap().is_empty());
    assert!(
        journal
            .get("primary", "alice", "stable")
            .await
            .unwrap()
            .is_none()
    );
    grants
        .replace(
            &actor(),
            "primary",
            Grant::new(0, Budget::new("quota").unwrap())
                .allow(Action::SendMessage)
                .chats(ChatScope::All),
        )
        .unwrap();
    let receipt = run.execute(&actor(), &call, None).await.unwrap();
    assert_eq!(
        receipt.operation().unwrap().state(),
        OperationState::Succeeded
    );
    let serialized = serde_json::to_string(&receipt).unwrap();
    assert!(!serialized.contains("PRIVATE_"));
    assert!(!format!("{receipt:?} {run:?}").contains("-1001"));
    assert_eq!(
        run.execute(&actor(), &call, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
    grants.revoke(&actor(), "primary").unwrap();
    assert!(run.execute(&actor(), &call, None).await.is_err());
    assert_eq!(mock.requests().unwrap().len(), 2);
}
#[tokio::test]
async fn terminal_unknown_refused_and_wrong_target_never_replay_or_disclose_remote_content() {
    for (step, expected) in [
        (Step::timeout("sendMessage"), OperationState::Unknown),
        (
            Step::reply(
                "sendMessage",
                MockResponse::json(
                    json!({"ok":false,"error_code":429,"description":"PRIVATE_REMOTE","parameters":{"retry_after":1}}),
                ),
            ),
            OperationState::Rejected,
        ),
        (sent(999), OperationState::Unknown),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("agent.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let mock = MockTransport::new([identity(), step]).unwrap();
        let run = executor(journal, policy(), &mock).await;
        let call = request("terminal", "PRIVATE_TEXT");
        for _ in 0..2 {
            let receipt = run.execute(&actor(), &call, None).await.unwrap();
            assert_eq!(receipt.operation().unwrap().state(), expected);
            assert!(
                !serde_json::to_string(&receipt)
                    .unwrap()
                    .contains("PRIVATE_")
            );
        }
        assert_eq!(mock.requests().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn restart_returns_success_without_identity_or_send_rpc_and_changed_intent_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let first = MockTransport::new([identity(), sent(-1001)]).unwrap();
    executor(journal.clone(), policy(), &first)
        .await
        .execute(&actor(), &request("stable", "original"), None)
        .await
        .unwrap();
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let no_network = MockTransport::new([]).unwrap();
    let run = executor(journal, policy(), &no_network).await;
    assert_eq!(
        run.execute(&actor(), &request("stable", "original"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    let changed = run
        .execute(&actor(), &request("stable", "changed"), None)
        .await
        .unwrap_err();
    assert_eq!(changed.kind(), ExecutorErrorKind::Conflict);
    assert_eq!(changed.outcome(), Some(EffectOutcome::NotSent));
    assert!(no_network.requests().unwrap().is_empty());
}
