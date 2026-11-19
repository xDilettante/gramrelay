#![cfg(feature = "executor")]
use serde_json::{Value, json};
use std::time::Duration;
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    executor::{
        Action, Budget, CallbackClockPolicy, ChatId, ChatScope, Executor, ExecutorErrorKind,
        ExecutorOptions, Grant, PolicyStore, Principal, Request,
    },
    journal::{Journal, JournalOptions, OperationState},
    updates::{SourceMode, StoredUpdate, UpdateFeed, UpdateOptions},
};
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn grant() -> Grant {
    Grant::new(0, Budget::new("quota").unwrap())
        .allow(Action::AnswerCallbackQuery)
        .allow(Action::ReadUpdates)
        .allow(Action::GetOperation)
        .chats(ChatScope::only([ChatId::new(1).unwrap()]))
}
fn call(tool: &str, key: Option<&str>, args: Value) -> Request {
    let mut value =
        json!({"schema_version":1,"profile":"primary","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        value["operation_id"] = json!(key);
    }
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn answer(key: &str, id: &str) -> Request {
    call(
        "answer_callback_query",
        Some(key),
        json!({"callback_query_id":id,"text":"PRIVATE_ANSWER","show_alert":true}),
    )
}
fn event(id: i64, query: &str, chat: Option<i64>) -> tg::Update {
    let mut callback = json!({"id":query,"from":{"id":5,"is_bot":false,"first_name":"Offline"},"chat_instance":"opaque","data":"PRIVATE_DATA"});
    if let Some(chat) = chat {
        callback["message"] = json!({"message_id":9,"date":1,"chat":{"id":chat,"type":"private"}});
    } else {
        callback["inline_message_id"] = json!("PRIVATE_INLINE");
    }
    serde_json::from_value(json!({"update_id":id,"callback_query":callback})).unwrap()
}
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ),
    )
}
fn accepted() -> Step {
    Step::reply(
        "answerCallbackQuery",
        MockResponse::json(json!({"ok":true,"result":true})),
    )
}
struct Fixture {
    directory: tempfile::TempDir,
    journal: Journal,
    feed: UpdateFeed,
    policy: PolicyStore,
    mock: MockTransport,
    executor: Executor,
}
impl Fixture {
    async fn new(steps: impl IntoIterator<Item = Step>, options: ExecutorOptions) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("agent.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
            .await
            .unwrap();
        let policy = PolicyStore::default();
        policy.replace(&actor(), "primary", grant()).unwrap();
        let mock = MockTransport::new(steps).unwrap();
        let executor = Executor::new(policy.clone(), options).unwrap();
        executor
            .register_profile(feed.clone(), mock.bot().unwrap())
            .unwrap();
        Self {
            directory,
            journal,
            feed,
            policy,
            mock,
            executor,
        }
    }
    async fn append(&self, update: tg::Update) -> StoredUpdate {
        let before = self
            .feed
            .read(None, 100)
            .await
            .unwrap()
            .next_cursor()
            .clone();
        let source = self.feed.claim_source(SourceMode::Webhook).await.unwrap();
        self.feed.append_webhook(&source, update).await.unwrap();
        self.feed.release_source(source).await.unwrap();
        self.feed.read(Some(&before), 1).await.unwrap().events()[0].clone()
    }
}

#[tokio::test]
async fn authorized_read_registers_a_callback_and_ack_receipt_is_minimal_and_durable() {
    let fixture = Fixture::new([identity(), accepted()], ExecutorOptions::default()).await;
    fixture.append(event(1, "PRIVATE_QUERY", Some(1))).await;
    let read = fixture
        .executor
        .execute(&actor(), &call("read_updates", None, json!({})), None)
        .await
        .unwrap();
    assert_eq!(
        read.updates().unwrap().events()[0].callback_query_id(),
        Some("PRIVATE_QUERY")
    );
    for _ in 0..2 {
        let result = fixture
            .executor
            .execute(&actor(), &answer("same-key", "PRIVATE_QUERY"), None)
            .await
            .unwrap();
        assert_eq!(
            result.operation().unwrap().state(),
            OperationState::Succeeded
        );
        assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
        assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE_"));
    }
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
    let params = fixture.mock.requests().unwrap()[1]
        .decode::<tg::api::methods::AnswerCallbackQuery>()
        .unwrap();
    assert_eq!(params.callback_query_id.as_str(), "PRIVATE_QUERY");
    assert_eq!(params.show_alert, Some(true));
    assert!(params.url.is_none());
    assert!(params.cache_time.is_none());
    let status = fixture
        .executor
        .execute(
            &actor(),
            &call("get_operation", None, json!({"operation_id":"same-key"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(status.operation().unwrap().acknowledged(), Some(true));
}

#[tokio::test]
async fn references_bind_principal_source_instance_and_durable_identity_across_principals() {
    let fixture = Fixture::new([identity(), accepted()], ExecutorOptions::default()).await;
    let stored = fixture.append(event(1, "query", Some(1))).await;
    fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    let bob = Principal::new("bob").unwrap();
    fixture.policy.replace(&bob, "primary", grant()).unwrap();
    assert!(
        fixture
            .executor
            .execute(&bob, &answer("bypass", "query"), None)
            .await
            .is_err()
    );
    let other = Fixture::new([], ExecutorOptions::default()).await;
    let foreign = other.append(event(1, "query-other", Some(1))).await;
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &foreign)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    fixture
        .executor
        .observe_callback(&bob, "primary", &stored)
        .unwrap();
    fixture
        .executor
        .execute(&actor(), &answer("first", "query"), None)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&bob, &answer("different", "query"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn rereading_never_refreshes_expiry_and_recovery_does_not_need_a_live_reference() {
    let fixture = Fixture::new(
        [identity(), accepted()],
        ExecutorOptions::default().callback_ttl(Duration::from_millis(1000)),
    )
    .await;
    let stored = fixture.append(event(1, "query", Some(1))).await;
    let first = fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    let again = fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    assert_eq!(first.expires_at_ms(), again.expires_at_ms());
    fixture
        .executor
        .execute(&actor(), &answer("stable", "query"), None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &stored)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceExpired
    );
    let error = fixture
        .executor
        .execute(&actor(), &answer("new-key", "query"), None)
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::Unknown));
    let status = fixture
        .executor
        .execute(
            &actor(),
            &call("get_operation", None, json!({"operation_id":"stable"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        status.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_cannot_be_bypassed_with_a_new_key_or_a_new_registry() {
    let fixture = Fixture::new(
        [identity(), Step::timeout("answerCallbackQuery")],
        ExecutorOptions::default().request_budget(Duration::from_millis(50)),
    )
    .await;
    let stored = fixture.append(event(1, "query", Some(1))).await;
    fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &answer("uncertain", "query"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Unknown
    );
    let fresh = Executor::new(fixture.policy.clone(), ExecutorOptions::default()).unwrap();
    fresh
        .register_profile(fixture.feed.clone(), fixture.mock.bot().unwrap())
        .unwrap();
    fresh
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    assert_eq!(
        fresh
            .execute(&actor(), &answer("bypass", "query"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn inline_requires_global_scope_and_composite_callbacks_never_issue_references() {
    let fixture = Fixture::new([], ExecutorOptions::default()).await;
    let inline = fixture.append(event(1, "inline", None)).await;
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &inline)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    fixture
        .policy
        .replace(&actor(), "primary", grant().unscoped_updates(true))
        .unwrap();
    fixture
        .executor
        .observe_callback(&actor(), "primary", &inline)
        .unwrap();
    let mut mixed = serde_json::to_value(event(2, "mixed", Some(1))).unwrap();
    mixed["callback_query"]["inline_message_id"] = json!("inline");
    let mixed = fixture.append(serde_json::from_value(mixed).unwrap()).await;
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &mixed)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidInput
    );
    let mut extended = serde_json::to_value(event(3, "extended", Some(1))).unwrap();
    extended["future_event"] = json!({"extra":true});
    let extended = fixture
        .append(serde_json::from_value(extended).unwrap())
        .await;
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &extended)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidInput
    );
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn a_full_reference_registry_omits_action_metadata_without_losing_readable_events() {
    let fixture = Fixture::new([], ExecutorOptions::default().max_callback_references(1)).await;
    fixture.append(event(1, "first", Some(1))).await;
    let second = fixture.append(event(2, "second", Some(1))).await;
    let read = fixture
        .executor
        .execute(&actor(), &call("read_updates", None, json!({})), None)
        .await
        .unwrap();
    assert_eq!(read.updates().unwrap().events().len(), 2);
    assert!(
        read.updates().unwrap().events()[0]
            .callback_query_id()
            .is_some()
    );
    assert!(
        read.updates().unwrap().events()[1]
            .callback_query_id()
            .is_none()
    );
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &second)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
}

#[tokio::test]
async fn journal_restart_fails_closed_without_monotonic_provenance_and_wall_mode_is_explicit() {
    let fixture = Fixture::new([], ExecutorOptions::default()).await;
    fixture.append(event(1, "query", Some(1))).await;
    fixture.journal.close().await.unwrap();
    let journal = Journal::open(
        fixture.directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let feed = UpdateFeed::open(journal, "primary", 0, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert!(
        !feed
            .append_webhook(&source, event(1, "query", Some(1)))
            .await
            .unwrap()
            .inserted()
    );
    feed.release_source(source).await.unwrap();
    let snapshot = feed.read(None, 1).await.unwrap().events()[0].clone();
    let strict = Executor::new(fixture.policy.clone(), ExecutorOptions::default()).unwrap();
    strict
        .register_profile(feed.clone(), fixture.mock.bot().unwrap())
        .unwrap();
    assert_eq!(
        strict
            .observe_callback(&actor(), "primary", &snapshot)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    let explicit = Executor::new(
        fixture.policy.clone(),
        ExecutorOptions::default().callback_clock_policy(CallbackClockPolicy::TrustedWallClock),
    )
    .unwrap();
    explicit
        .register_profile(feed, fixture.mock.bot().unwrap())
        .unwrap();
    assert!(
        explicit
            .observe_callback(&actor(), "primary", &snapshot)
            .is_ok()
    );
}

#[tokio::test]
async fn ingestion_monotonic_age_cannot_be_refreshed_by_rewriting_wall_time() {
    let fixture = Fixture::new(
        [],
        ExecutorOptions::default().callback_ttl(Duration::from_millis(40)),
    )
    .await;
    fixture.append(event(1, "query", Some(1))).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    rusqlite::Connection::open(fixture.directory.path().join("agent.sqlite"))
        .unwrap()
        .execute("UPDATE updates SET received_at_ms=?1", [now])
        .unwrap();
    let reread = fixture.feed.read(None, 1).await.unwrap().events()[0].clone();
    assert_eq!(
        fixture
            .executor
            .observe_callback(&actor(), "primary", &reread)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceExpired
    );
    let encoded = serde_json::to_string(&reread).unwrap();
    assert!(!encoded.contains("callback_received"));
    assert!(!encoded.contains("origin"));
}

#[tokio::test]
async fn polling_ingestion_also_provides_fresh_callback_provenance_without_network() {
    let fixture = Fixture::new([], ExecutorOptions::default()).await;
    let source = fixture
        .feed
        .claim_source(SourceMode::Polling)
        .await
        .unwrap();
    let checkpoint = fixture.feed.poll_checkpoint(&source).await.unwrap();
    fixture
        .feed
        .append_poll(&source, &checkpoint, vec![event(1, "query", Some(1))])
        .await
        .unwrap();
    fixture.feed.release_source(source).await.unwrap();
    let stored = fixture.feed.read(None, 1).await.unwrap().events()[0].clone();
    fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn reference_expiry_during_receipt_commit_requires_get_operation_for_disclosure() {
    let fixture = Fixture::new(
        [
            identity(),
            Step::reply(
                "answerCallbackQuery",
                MockResponse::json(json!({"ok":true,"result":true}))
                    .delay(Duration::from_millis(150)),
            ),
        ],
        ExecutorOptions::default().callback_ttl(Duration::from_millis(400)),
    )
    .await;
    let stored = fixture.append(event(1, "query", Some(1))).await;
    fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    let worker = fixture.executor.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(&actor(), &answer("late-receipt", "query"), None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !fixture
            .mock
            .requests()
            .unwrap()
            .iter()
            .any(|request| request.method() == "answerCallbackQuery")
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let blocker =
        rusqlite::Connection::open(fixture.directory.path().join("agent.sqlite")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    blocker.execute_batch("ROLLBACK").unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::ReferenceExpired);
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::Unknown));
    let result = fixture
        .executor
        .execute(
            &actor(),
            &call(
                "get_operation",
                None,
                json!({"operation_id":"late-receipt"}),
            ),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn confirmed_not_sent_retries_only_the_same_key_and_spends_another_admission() {
    let fixture = Fixture::new(
        [identity(), Step::timeout("getChat"), accepted()],
        ExecutorOptions::default(),
    )
    .await;
    fixture
        .policy
        .replace(&actor(), "primary", grant().allow(Action::GetMe))
        .unwrap();
    let bot = fixture.mock.bot_builder().max_in_flight(1).build().unwrap();
    let executor = Executor::new(fixture.policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(fixture.feed.clone(), bot.clone())
        .unwrap();
    executor
        .execute(&actor(), &call("get_me", None, json!({})), None)
        .await
        .unwrap();
    let stored = fixture.append(event(1, "query", Some(1))).await;
    executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    let blocked = tokio::spawn(async move { bot.api().get_chat(tg::ChatId::from(1).into()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !fixture
            .mock
            .requests()
            .unwrap()
            .iter()
            .any(|request| request.method() == "getChat")
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let failed = executor
        .execute(&actor(), &answer("same-key", "query"), None)
        .await
        .unwrap();
    assert_eq!(
        failed.operation().unwrap().state(),
        OperationState::Prepared
    );
    assert_eq!(
        failed.operation().unwrap().failure().unwrap().outcome(),
        "not_sent"
    );
    assert_eq!(
        executor
            .execute(&actor(), &answer("fresh-key", "query"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert!(
        fixture
            .journal
            .get("primary", "alice", "fresh-key")
            .await
            .unwrap()
            .is_none()
    );
    blocked.abort();
    assert!(blocked.await.unwrap_err().is_cancelled());
    let success = executor
        .execute(&actor(), &answer("same-key", "query"), None)
        .await
        .unwrap();
    assert_eq!(
        success.operation().unwrap().state(),
        OperationState::Succeeded
    );
    let attempts: i64 = rusqlite::Connection::open(fixture.directory.path().join("agent.sqlite"))
        .unwrap()
        .query_row("SELECT write_attempts FROM executor_budgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(fixture.mock.requests().unwrap().len(), 3);
}

#[tokio::test]
async fn host_confirmation_is_exact_and_cannot_outlive_the_reference() {
    let fixture = Fixture::new([identity(), accepted()], ExecutorOptions::default()).await;
    let stored = fixture.append(event(1, "query", Some(1))).await;
    fixture
        .executor
        .observe_callback(&actor(), "primary", &stored)
        .unwrap();
    fixture
        .policy
        .replace(
            &actor(),
            "primary",
            grant().require_approval(Action::AnswerCallbackQuery),
        )
        .unwrap();
    let request = answer("confirmed", "query");
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &request, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert_eq!(
        fixture
            .executor
            .approve(&actor(), &request, now + 60000)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    let approval = fixture
        .executor
        .approve(&actor(), &request, now + 5000)
        .await
        .unwrap();
    assert!(
        fixture
            .executor
            .execute(&actor(), &answer("other-key", "query"), Some(&approval))
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &request, Some(&approval))
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}
