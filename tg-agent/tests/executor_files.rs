#![cfg(feature = "executor")]
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tg::test_kit::MockTransport;
use tg::test_kit::{MockResponse, Step};
use tg_agent::executor::{Action, Budget, ChatId, ChatScope, Grant, Request};
use tg_agent::{
    executor::{Executor, ExecutorErrorKind, ExecutorOptions, FileLimits, PolicyStore, Principal},
    journal::{Journal, JournalOptions},
    updates::{UpdateFeed, UpdateOptions},
};
fn expiry() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60000
}
async fn executor(journal: &Journal, limits: FileLimits) -> Executor {
    let executor = Executor::new(
        PolicyStore::default(),
        ExecutorOptions::default().file_limits(limits),
    )
    .unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            MockTransport::new([]).unwrap().bot().unwrap(),
        )
        .unwrap();
    executor
}
#[tokio::test]
async fn immutable_metadata_survives_discard_and_only_matching_bytes_restore() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("files.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let executor = executor(&journal, FileLimits::default()).await;
    let principal = Principal::new("alice").unwrap();
    let file = executor
        .register_file(
            &principal,
            "primary",
            "PRIVATE.bin",
            vec![0, 255, 3],
            expiry(),
        )
        .await
        .unwrap();
    assert_eq!(file.byte_size(), 3);
    assert_eq!(file.filename(), "PRIVATE.bin");
    assert!(!format!("{file:?}").contains("PRIVATE"));
    assert!(
        executor
            .discard_file(&principal, "primary", file.handle())
            .unwrap()
    );
    assert_eq!(
        executor
            .restore_file(&principal, "primary", file.handle(), vec![0, 254, 3])
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    let restored = executor
        .restore_file(&principal, "primary", file.handle(), vec![0, 255, 3])
        .await
        .unwrap();
    assert_eq!(restored.sha256(), file.sha256());
    let other = executor
        .register_file(
            &principal,
            "primary",
            "PRIVATE.bin",
            vec![0, 255, 3],
            expiry(),
        )
        .await
        .unwrap();
    assert_ne!(other.handle(), file.handle());
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM executor_files", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    journal.close().await.unwrap();
}
#[tokio::test]
async fn memory_limits_charge_retained_capacity_and_no_live_snapshot_is_evicted() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("files.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let limits = FileLimits {
        max_file_bytes: 4,
        max_snapshot_bytes: 4,
        max_snapshots: 1,
        max_upload_bytes: 4,
    };
    let executor = executor(&journal, limits).await;
    let principal = Principal::new("alice").unwrap();
    let mut oversized = Vec::with_capacity(16);
    oversized.push(1);
    assert_eq!(
        executor
            .register_file(&principal, "primary", "big.bin", oversized, expiry())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    let file = executor
        .register_file(&principal, "primary", "one.bin", vec![1], expiry())
        .await
        .unwrap();
    assert_eq!(
        executor
            .register_file(&principal, "primary", "two.bin", vec![], expiry())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    executor
        .discard_file(&principal, "primary", file.handle())
        .unwrap();
    executor
        .register_file(&principal, "primary", "empty.bin", vec![], expiry())
        .await
        .unwrap();
    journal.close().await.unwrap();
}
#[tokio::test]
async fn restart_needs_explicit_restoration_and_wrong_owner_cannot_restore_or_discard() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("files.sqlite");
    let principal = Principal::new("alice").unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let first = executor(&journal, FileLimits::default()).await;
    let file = first
        .register_file(&principal, "primary", "data.bin", vec![2, 3], expiry())
        .await
        .unwrap();
    journal.close().await.unwrap();
    drop(first);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = executor(&journal, FileLimits::default()).await;
    assert!(
        !second
            .discard_file(&principal, "primary", file.handle())
            .unwrap()
    );
    assert_eq!(
        second
            .restore_file(
                &Principal::new("bob").unwrap(),
                "primary",
                file.handle(),
                vec![2, 3]
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    second
        .restore_file(&principal, "primary", file.handle(), vec![2, 3])
        .await
        .unwrap();
    assert!(
        !second
            .discard_file(&Principal::new("bob").unwrap(), "primary", file.handle())
            .unwrap()
    );
    assert!(
        second
            .discard_file(&principal, "primary", file.handle())
            .unwrap()
    );
    journal.close().await.unwrap();
}
#[tokio::test]
async fn invalid_names_expiry_and_durable_capacity_fail_without_overwriting_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("files.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_file_handles(1))
        .await
        .unwrap();
    let executor = executor(&journal, FileLimits::default()).await;
    let principal = Principal::new("alice").unwrap();
    for name in ["", "../data", "data\\file", ".", "..", "line\nfile"] {
        assert_eq!(
            executor
                .register_file(&principal, "primary", name, vec![1], expiry())
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::InvalidInput
        );
    }
    assert_eq!(
        executor
            .register_file(&principal, "primary", "old.bin", vec![1], 1)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileExpired
    );
    let file = executor
        .register_file(&principal, "primary", "one.bin", vec![1], expiry())
        .await
        .unwrap();
    executor
        .discard_file(&principal, "primary", file.handle())
        .unwrap();
    assert_eq!(
        executor
            .register_file(&principal, "primary", "two.bin", vec![2], expiry())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    executor
        .restore_file(&principal, "primary", file.handle(), vec![1])
        .await
        .unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn document_upload_is_exact_scoped_once_and_cached_without_resident_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("document.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let mock=MockTransport::new([
        Step::reply("getMe",MockResponse::json(json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}))),
        Step::reply("sendDocument",MockResponse::json(json!({"ok":true,"result":{"message_id":9,"date":1,"chat":{"id":1,"type":"private"},"document":{"file_id":"PRIVATE_FILE_ID","file_unique_id":"PRIVATE_UNIQUE"}}}))),
    ]).unwrap();
    let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            mock.bot().unwrap(),
        )
        .unwrap();
    let file = executor
        .register_file(
            &principal,
            "primary",
            "PRIVATE.bin",
            vec![0, 255, 3],
            expiry(),
        )
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            Grant::new(0, Budget::new("budget").unwrap())
                .allow(Action::SendDocument)
                .allow(Action::GetOperation)
                .chats(ChatScope::only([ChatId::new(1).unwrap()]))
                .files([file.handle()]),
        )
        .unwrap();
    let request=Request::from_json(&serde_json::to_vec(&json!({"schema_version":1,"profile":"primary","operation_id":"document-1","call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":file.handle(),"caption":"PRIVATE_CAPTION"}}})).unwrap()).unwrap();
    executor.dry_run(&principal, &request, None).await.unwrap();
    assert!(mock.requests().unwrap().is_empty());
    let result = executor.execute(&principal, &request, None).await.unwrap();
    let output = serde_json::to_string(&result).unwrap();
    assert!(output.contains("succeeded"));
    assert!(!output.contains("PRIVATE"));
    executor
        .discard_file(&principal, "primary", file.handle())
        .unwrap();
    executor.execute(&principal, &request, None).await.unwrap();
    let requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].body().windows(3).any(|v| v == [0, 255, 3]));
    let body = String::from_utf8_lossy(requests[1].body());
    assert!(body.contains("filename=\"PRIVATE.bin\""));
    assert!(body.contains("PRIVATE_CAPTION"));
    assert!(!body.contains("allow_paid_broadcast"));
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT upload_bytes FROM executor_budgets", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        3
    );
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

fn document_request(handle: &str, key: &str) -> Request {
    Request::from_json(&serde_json::to_vec(&json!({"schema_version":1,"profile":"primary","operation_id":key,"call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":handle}}})).unwrap()).unwrap()
}
fn document_grant(handle: &str, budget: Budget) -> Grant {
    Grant::new(0, budget)
        .allow(Action::SendDocument)
        .allow(Action::GetOperation)
        .chats(ChatScope::only([ChatId::new(1).unwrap()]))
        .files([handle])
}
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ),
    )
}
fn sent(chat: i64) -> Step {
    Step::reply(
        "sendDocument",
        MockResponse::json(
            json!({"ok":true,"result":{"message_id":9,"date":1,"chat":{"id":chat,"type":"private"}}}),
        ),
    )
}
async fn document_executor(
    journal: &Journal,
    policy: &PolicyStore,
    mock: &MockTransport,
) -> Executor {
    let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            mock.bot().unwrap(),
        )
        .unwrap();
    executor
}
fn operation_request(key: &str) -> Request {
    Request::from_json(&serde_json::to_vec(&json!({"schema_version":1,"profile":"primary","call":{"tool":"get_operation","arguments":{"operation_id":key}}})).unwrap()).unwrap()
}

#[tokio::test]
async fn document_restart_reads_cache_without_rpc_and_new_upload_requires_identical_restoration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.sqlite");
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let first_mock = MockTransport::new([identity(), sent(1)]).unwrap();
    let first = document_executor(&journal, &policy, &first_mock).await;
    let file = first
        .register_file(&principal, "primary", "one.bin", vec![1, 2], expiry())
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            document_grant(file.handle(), Budget::new("budget").unwrap()),
        )
        .unwrap();
    first
        .execute(
            &principal,
            &document_request(file.handle(), "original"),
            None,
        )
        .await
        .unwrap();
    journal.close().await.unwrap();
    drop(first);
    first_mock.assert_finished().unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second_mock = MockTransport::new([identity(), sent(1)]).unwrap();
    let second = document_executor(&journal, &policy, &second_mock).await;
    second
        .execute(
            &principal,
            &document_request(file.handle(), "original"),
            None,
        )
        .await
        .unwrap();
    let new = document_request(file.handle(), "new-intent");
    assert_eq!(
        second
            .execute(&principal, &new, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileUnavailable
    );
    assert!(second_mock.requests().unwrap().is_empty());
    second
        .restore_file(&principal, "primary", file.handle(), vec![1, 2])
        .await
        .unwrap();
    second.execute(&principal, &new, None).await.unwrap();
    assert_eq!(second_mock.requests().unwrap().len(), 2);
    policy
        .replace(
            &principal,
            "primary",
            Grant::new(0, Budget::new("budget").unwrap())
                .allow(Action::GetOperation)
                .allow(Action::SendDocument)
                .chats(ChatScope::All),
        )
        .unwrap();
    assert_eq!(
        second
            .execute(&principal, &operation_request("original"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    second_mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn document_upload_quota_and_begin_are_atomic_and_not_sent_retry_spends_once() {
    use tg_agent::executor::BudgetLimits;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let mock = MockTransport::new([identity(), sent(1)]).unwrap();
    let executor = document_executor(&journal, &policy, &mock).await;
    let file = executor
        .register_file(&principal, "primary", "three.bin", vec![1, 2, 3], expiry())
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            document_grant(
                file.handle(),
                Budget::new("budget").unwrap().limits(BudgetLimits {
                    upload_bytes: 2,
                    ..Default::default()
                }),
            ),
        )
        .unwrap();
    let request = document_request(file.handle(), "quota-retry");
    assert_eq!(
        executor
            .execute(&principal, &request, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT write_attempts,upload_bytes FROM executor_budgets",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (0, 0)
    );
    assert_eq!(mock.requests().unwrap().len(), 1);
    policy
        .replace(
            &principal,
            "primary",
            document_grant(
                file.handle(),
                Budget::new("budget").unwrap().limits(BudgetLimits {
                    upload_bytes: 3,
                    ..Default::default()
                }),
            ),
        )
        .unwrap();
    executor.execute(&principal, &request, None).await.unwrap();
    executor.execute(&principal, &request, None).await.unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT write_attempts,upload_bytes FROM executor_budgets",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (1, 3)
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn expired_files_prevent_new_upload_but_retained_receipts_stay_currently_authorized() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("expiry.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let mock = MockTransport::new([identity(), sent(1)]).unwrap();
    let executor = document_executor(&journal, &policy, &mock).await;
    let file = executor
        .register_file(&principal, "primary", "one.bin", vec![1], expiry() - 59000)
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            document_grant(file.handle(), Budget::new("budget").unwrap()),
        )
        .unwrap();
    let request = document_request(file.handle(), "retained");
    executor.execute(&principal, &request, None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(
        executor
            .execute(
                &principal,
                &document_request(file.handle(), "expired-new"),
                None
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileExpired
    );
    assert_eq!(executor.prune_files().unwrap(), 1);
    for request in [request, operation_request("retained")] {
        let result = executor.execute(&principal, &request, None).await.unwrap();
        assert_eq!(
            result.operation().unwrap().state(),
            tg_agent::journal::OperationState::Succeeded
        );
    }
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn uncertain_upload_and_wrong_target_are_terminal_even_after_snapshot_discard() {
    for response in [Step::partial_upload("sendDocument", 23), sent(2)] {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(dir.path().join("unknown.sqlite"), JournalOptions::default())
            .await
            .unwrap();
        let policy = PolicyStore::default();
        let principal = Principal::new("alice").unwrap();
        let mock = MockTransport::new([identity(), response]).unwrap();
        let executor = document_executor(&journal, &policy, &mock).await;
        let file = executor
            .register_file(&principal, "primary", "one.bin", vec![1], expiry())
            .await
            .unwrap();
        policy
            .replace(
                &principal,
                "primary",
                document_grant(file.handle(), Budget::new("budget").unwrap()),
            )
            .unwrap();
        let request = document_request(file.handle(), "uncertain");
        let result = executor.execute(&principal, &request, None).await.unwrap();
        assert_eq!(
            result.operation().unwrap().state(),
            tg_agent::journal::OperationState::Unknown
        );
        executor
            .discard_file(&principal, "primary", file.handle())
            .unwrap();
        let result = executor.execute(&principal, &request, None).await.unwrap();
        assert_eq!(
            result.operation().unwrap().state(),
            tg_agent::journal::OperationState::Unknown
        );
        assert!(result.operation().unwrap().message().is_none());
        assert_eq!(mock.requests().unwrap().len(), 2);
        mock.assert_finished().unwrap();
        journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn document_approval_binds_the_exact_handle_payload_and_key() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        dir.path().join("approval.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let mock = MockTransport::new([identity(), sent(1)]).unwrap();
    let executor = document_executor(&journal, &policy, &mock).await;
    let file = executor
        .register_file(&principal, "primary", "one.bin", vec![1], expiry())
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            document_grant(file.handle(), Budget::new("budget").unwrap())
                .require_approval(Action::SendDocument),
        )
        .unwrap();
    let request = document_request(file.handle(), "approved");
    assert_eq!(
        executor
            .execute(&principal, &request, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = executor
        .approve(&principal, &request, expiry())
        .await
        .unwrap();
    assert_eq!(
        executor
            .execute(
                &principal,
                &document_request(file.handle(), "changed-key"),
                Some(&approval)
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    assert!(mock.requests().unwrap().is_empty());
    executor
        .execute(&principal, &request, Some(&approval))
        .await
        .unwrap();
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn independent_connections_cannot_exceed_the_shared_metadata_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite");
    let first = Journal::open(&path, JournalOptions::default().max_file_handles(1))
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default().max_file_handles(1))
        .await
        .unwrap();
    let a = executor(&first, FileLimits::default()).await;
    let b = executor(&second, FileLimits::default()).await;
    let principal = Principal::new("alice").unwrap();
    let (one, two) = tokio::join!(
        a.register_file(&principal, "primary", "one.bin", vec![1], expiry()),
        b.register_file(&principal, "primary", "two.bin", vec![2], expiry())
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert_eq!(
        one.err().or_else(|| two.err()).unwrap().kind(),
        ExecutorErrorKind::Capacity
    );
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM executor_files", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    first.close().await.unwrap();
    second.close().await.unwrap();
}

#[tokio::test]
async fn a_confirmed_sdk_not_sent_retry_spends_upload_bytes_again_on_the_same_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-sent.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    let principal = Principal::new("alice").unwrap();
    let mock = MockTransport::new([identity(), Step::timeout("getChat"), sent(1)]).unwrap();
    let bot = mock.bot_builder().max_in_flight(1).build().unwrap();
    let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            bot.clone(),
        )
        .unwrap();
    let file = executor
        .register_file(&principal, "primary", "two.bin", vec![1, 2], expiry())
        .await
        .unwrap();
    policy
        .replace(
            &principal,
            "primary",
            document_grant(file.handle(), Budget::new("budget").unwrap()).allow(Action::GetMe),
        )
        .unwrap();
    let me = Request::from_json(
        br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":{}}}"#,
    )
    .unwrap();
    executor.execute(&principal, &me, None).await.unwrap();
    let blocker = tokio::spawn(async move { bot.api().get_chat(tg::ChatId::from(1).into()).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while mock.requests().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = document_request(file.handle(), "same-key");
    let receipt = executor.execute(&principal, &request, None).await.unwrap();
    assert_eq!(
        receipt.operation().unwrap().state(),
        tg_agent::journal::OperationState::Prepared
    );
    assert_eq!(
        receipt.operation().unwrap().failure().unwrap().outcome(),
        "not_sent"
    );
    blocker.abort();
    let _ = blocker.await;
    executor.execute(&principal, &request, None).await.unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT write_attempts,upload_bytes FROM executor_budgets",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (2, 4)
    );
    assert_eq!(mock.requests().unwrap().len(), 3);
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}
