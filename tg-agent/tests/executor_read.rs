#![cfg(feature = "executor")]
use serde_json::{Value, json};
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorErrorKind, ExecutorOptions, Grant,
        PolicyStore, Principal, Request,
    },
    journal::{Journal, JournalOptions, OperationState},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn grant() -> Grant {
    Grant::new(0, Budget::new("durable-read-budget").unwrap())
        .allow(Action::GetMe)
        .allow(Action::GetChat)
        .allow(Action::ReadUpdates)
        .allow(Action::GetOperation)
        .allow(Action::SendMessage)
        .chats(ChatScope::only([ChatId::new(1).unwrap()]))
}
fn call(tool: &str, args: Value) -> Request {
    let mut value =
        json!({"schema_version":1,"profile":"primary","call":{"tool":tool,"arguments":args}});
    if tool == "send_message" {
        value["operation_id"] = json!("stable-send");
    }
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"PRIVATE_NAME","username":"offline_bot","future_private":"PRIVATE_FUTURE"}}),
        ),
    )
}
fn sent() -> Step {
    Step::reply(
        "sendMessage",
        MockResponse::json(
            json!({"ok":true,"result":{"message_id":9,"date":1,"chat":{"id":1,"type":"private"}}}),
        ),
    )
}
fn chat_reply(id: i64, extra: Value) -> Step {
    let info = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(id),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    );
    let mut result = serde_json::to_value(info).unwrap();
    result
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Step::reply(
        "getChat",
        MockResponse::json(json!({"ok":true,"result":result})),
    )
}
fn message(id: i64, chat: i64, text: &str) -> tg::Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"},"text":text,"future_message":{"retained":true}}})).unwrap()
}
struct Fixture {
    _directory: tempfile::TempDir,
    journal: Journal,
    feed: UpdateFeed,
    policy: PolicyStore,
    mock: MockTransport,
    executor: Executor,
}
impl Fixture {
    async fn new(steps: impl IntoIterator<Item = Step>) -> Self {
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
        let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
        executor
            .register_profile(feed.clone(), mock.bot().unwrap())
            .unwrap();
        Self {
            _directory: directory,
            journal,
            feed,
            policy,
            mock,
            executor,
        }
    }
    async fn append(&self, events: Vec<tg::Update>) {
        let lease = self.feed.claim_source(SourceMode::Webhook).await.unwrap();
        for event in events {
            self.feed.append_webhook(&lease, event).await.unwrap();
        }
        self.feed.release_source(lease).await.unwrap();
    }
}

#[tokio::test]
async fn get_me_and_get_chat_share_verified_identity_and_allowlisted_projections() {
    let fixture = Fixture::new([identity(),chat_reply(1,json!({"first_name":"PRIVATE_PERSON","username":"known_user","invite_link":"PRIVATE_LINK","future_private":"PRIVATE_FUTURE"}))]).await;
    let chat = fixture
        .executor
        .execute(&actor(), &call("get_chat", json!({"chat_id":"1"})), None)
        .await
        .unwrap();
    assert_eq!(chat.chat().unwrap().chat_id(), ChatId::new(1).unwrap());
    assert!(!serde_json::to_string(&chat).unwrap().contains("PRIVATE_"));
    let bot = fixture
        .executor
        .execute(&actor(), &call("get_me", json!({})), None)
        .await
        .unwrap();
    assert_eq!(bot.bot().unwrap().bot_id(), "0");
    assert!(!serde_json::to_string(&bot).unwrap().contains("PRIVATE_"));
    assert!(!format!("{chat:?} {bot:?}").contains("known_user"));
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn forbidden_chat_is_local_and_a_mismatched_response_target_is_never_disclosed() {
    let fixture = Fixture::new([identity(), chat_reply(2, json!({"title":"PRIVATE_OTHER"}))]).await;
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &call("get_chat", json!({"chat_id":"2"})), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert!(fixture.mock.requests().unwrap().is_empty());
    let error = fixture
        .executor
        .execute(&actor(), &call("get_chat", json!({"chat_id":"1"})), None)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::InvalidResponse);
    assert!(!format!("{error:?}").contains("PRIVATE_OTHER"));
}

#[tokio::test]
async fn get_operation_and_dry_run_refuse_existing_manual_rows_without_persisted_scope() {
    use tg_agent::journal::OperationRequest;
    let fixture = Fixture::new([]).await;
    fixture
        .journal
        .prepare(
            OperationRequest::new(
                "primary",
                "alice",
                "manual",
                "send_message",
                &json!({"chat_id":"1","text":"legacy"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let lookup = call("get_operation", json!({"operation_id":"manual"}));
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(
        fixture
            .executor
            .dry_run(&actor(), &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn get_operation_uses_persisted_owner_original_action_and_current_chat_scope_without_rpc() {
    let fixture = Fixture::new([identity(), sent()]).await;
    fixture
        .executor
        .execute(
            &actor(),
            &call("send_message", json!({"chat_id":"1","text":"hello"})),
            None,
        )
        .await
        .unwrap();
    let lookup = call("get_operation", json!({"operation_id":"stable-send"}));
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    let bob = Principal::new("bob").unwrap();
    fixture
        .policy
        .replace(
            &actor(),
            "primary",
            grant().require_approval(Action::SendMessage),
        )
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    fixture.policy.replace(&bob, "primary", grant()).unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&bob, &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    let no_original_action = Grant::new(0, Budget::new("durable-read-budget").unwrap())
        .allow(Action::GetOperation)
        .chats(ChatScope::All);
    fixture
        .policy
        .replace(&actor(), "primary", no_original_action)
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    fixture
        .policy
        .replace(&actor(), "primary", grant().chats(ChatScope::only([])))
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn reference_dry_run_and_host_approval_do_not_refresh_state_or_spend_quota() {
    let fixture = Fixture::new([identity(), sent()]).await;
    fixture
        .executor
        .execute(
            &actor(),
            &call("send_message", json!({"chat_id":"1","text":"hello"})),
            None,
        )
        .await
        .unwrap();
    fixture
        .policy
        .replace(
            &actor(),
            "primary",
            grant().require_approval(Action::GetOperation),
        )
        .unwrap();
    let lookup = call("get_operation", json!({"operation_id":"stable-send"}));
    assert_eq!(
        fixture
            .executor
            .dry_run(&actor(), &lookup, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let approval = fixture
        .executor
        .approve(&actor(), &lookup, now + 10000)
        .await
        .unwrap();
    let before =
        rusqlite::Connection::open(fixture._directory.path().join("agent.sqlite")).unwrap();
    let usage = || {
        before
            .query_row(
                "SELECT invocations FROM executor_budgets WHERE principal='alice'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let start = usage();
    fixture
        .executor
        .dry_run(&actor(), &lookup, Some(&approval))
        .await
        .unwrap();
    assert_eq!(usage(), start);
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &lookup, Some(&approval))
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(usage(), start + 1);
}

#[tokio::test]
async fn scoped_read_stops_cursor_at_last_considered_event_and_preserves_lossless_untrusted_content()
 {
    let fixture = Fixture::new([]).await;
    fixture
        .append(vec![
            message(1, 2, "PRIVATE_OTHER"),
            message(2, 1, "allowed-first"),
            message(3, 2, "PRIVATE_OTHER"),
            message(4, 1, "allowed-second"),
        ])
        .await;
    let first = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({"limit":1})), None)
        .await
        .unwrap();
    let page = first.updates().unwrap();
    assert_eq!(page.events().len(), 1);
    assert_eq!(page.events()[0].update().update_id, 2);
    assert!(page.has_more());
    let value = serde_json::to_value(&first).unwrap();
    assert_eq!(value["updates"]["events"][0]["untrusted"], true);
    let raw: Value = serde_json::from_str(
        value["updates"]["events"][0]["telegram_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(raw["message"]["future_message"]["retained"], true);
    assert!(!value.to_string().contains("PRIVATE_OTHER"));
    let second = fixture
        .executor
        .execute(
            &actor(),
            &call(
                "read_updates",
                json!({"cursor":page.next_cursor().to_string(),"limit":1}),
            ),
            None,
        )
        .await
        .unwrap();
    assert_eq!(second.updates().unwrap().events()[0].update().update_id, 4);
    assert!(!second.updates().unwrap().has_more());
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_guest_and_top_level_extension_data_require_explicit_unscoped_grant() {
    let fixture = Fixture::new([]).await;
    let mut extended = serde_json::to_value(message(1, 1, "PRIVATE_EXTENSION")).unwrap();
    extended["new_event"] = json!({"private":"PRIVATE_UNKNOWN"});
    let guest = json!({"update_id":2,"guest_message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_GUEST"}});
    fixture
        .append(vec![
            serde_json::from_value(extended).unwrap(),
            serde_json::from_value(guest).unwrap(),
            serde_json::from_value(json!({"update_id":3,"new_event":{"private":"PRIVATE_FUTURE"}}))
                .unwrap(),
            message(4, 1, "ordinary"),
        ])
        .await;
    let initial = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert_eq!(initial.updates().unwrap().events().len(), 1);
    assert!(
        !serde_json::to_string(&initial)
            .unwrap()
            .contains("PRIVATE_")
    );
    fixture
        .policy
        .replace(&actor(), "primary", grant().unscoped_updates(true))
        .unwrap();
    let all = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert_eq!(all.updates().unwrap().events().len(), 4);
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn read_budget_does_not_skip_an_oversized_first_row_or_materialize_a_whole_large_feed() {
    let fixture = Fixture::new([]).await;
    let first = message(1, 1, "a");
    let size = serde_json::to_vec(&first).unwrap().len();
    fixture.append(vec![first, message(2, 1, "b")]).await;
    let bounded = Executor::new(
        fixture.policy.clone(),
        ExecutorOptions::default().max_read_bytes(size),
    )
    .unwrap();
    bounded
        .register_profile(fixture.feed.clone(), fixture.mock.bot().unwrap())
        .unwrap();
    let one = bounded
        .execute(&actor(), &call("read_updates", json!({"limit":100})), None)
        .await
        .unwrap();
    assert_eq!(one.updates().unwrap().events().len(), 1);
    assert!(one.updates().unwrap().has_more());
    let two = bounded
        .execute(
            &actor(),
            &call(
                "read_updates",
                json!({"cursor":one.updates().unwrap().next_cursor().to_string()}),
            ),
            None,
        )
        .await
        .unwrap();
    assert_eq!(two.updates().unwrap().events()[0].update().update_id, 2);
    let too_small = Executor::new(
        fixture.policy.clone(),
        ExecutorOptions::default().max_read_bytes(size - 1),
    )
    .unwrap();
    too_small
        .register_profile(fixture.feed.clone(), fixture.mock.bot().unwrap())
        .unwrap();
    assert_eq!(
        too_small
            .execute(&actor(), &call("read_updates", json!({})), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &call("read_updates", json!({})), None)
            .await
            .unwrap()
            .updates()
            .unwrap()
            .events()[0]
            .update()
            .update_id,
        1
    );
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn a_full_forbidden_scan_advances_one_page_and_current_permissions_filter_reused_cursors() {
    let fixture = Fixture::new([]).await;
    fixture
        .append(
            (1..=100)
                .map(|id| message(id, 2, "PRIVATE_OTHER"))
                .chain([message(101, 1, "allowed")])
                .collect(),
        )
        .await;
    let first = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert!(first.updates().unwrap().events().is_empty());
    assert!(first.updates().unwrap().has_more());
    let cursor = first.updates().unwrap().next_cursor().to_string();
    let second = fixture
        .executor
        .execute(
            &actor(),
            &call("read_updates", json!({"cursor":cursor,"limit":1})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        second.updates().unwrap().events()[0].update().update_id,
        101
    );
    fixture
        .policy
        .replace(
            &actor(),
            "primary",
            grant().chats(ChatScope::only([ChatId::new(2).unwrap()])),
        )
        .unwrap();
    let narrowed = fixture
        .executor
        .execute(
            &actor(),
            &call("read_updates", json!({"cursor":cursor})),
            None,
        )
        .await
        .unwrap();
    assert!(narrowed.updates().unwrap().events().is_empty());
    assert!(
        !serde_json::to_string(&narrowed)
            .unwrap()
            .contains("allowed")
    );
}

#[tokio::test]
async fn expired_and_foreign_cursors_are_local_and_dry_run_validates_position_without_consumption()
{
    let fixture = Fixture::new([]).await;
    let old = fixture
        .feed
        .read(None, 1)
        .await
        .unwrap()
        .next_cursor()
        .to_string();
    fixture.append(vec![message(1, 1, "expired")]).await;
    let connection =
        rusqlite::Connection::open(fixture._directory.path().join("agent.sqlite")).unwrap();
    connection
        .execute("UPDATE updates SET expires_at_ms=0", [])
        .unwrap();
    fixture.journal.prune_updates(10).await.unwrap();
    let read = call("read_updates", json!({"cursor":old}));
    assert_eq!(
        fixture
            .executor
            .dry_run(&actor(), &read, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::CursorExpired
    );
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &read, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::CursorExpired
    );
    let foreign = UpdateFeed::open(
        fixture.journal.clone(),
        "other",
        1,
        UpdateOptions::default(),
    )
    .await
    .unwrap()
    .read(None, 1)
    .await
    .unwrap()
    .next_cursor()
    .to_string();
    assert_eq!(
        fixture
            .executor
            .execute(
                &actor(),
                &call("read_updates", json!({"cursor":foreign})),
                None
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert!(fixture.mock.requests().unwrap().is_empty());
}

#[tokio::test]
async fn dry_run_of_an_expired_receipt_keeps_its_state_result_and_revision_unchanged() {
    let fixture = Fixture::new([identity(), sent()]).await;
    fixture
        .executor
        .execute(
            &actor(),
            &call("send_message", json!({"chat_id":"1","text":"hello"})),
            None,
        )
        .await
        .unwrap();
    let connection =
        rusqlite::Connection::open(fixture._directory.path().join("agent.sqlite")).unwrap();
    connection
        .execute("UPDATE operations SET expires_at_ms=0", [])
        .unwrap();
    let snapshot = || {
        connection.query_row("SELECT revision,expired,result_json FROM operations WHERE operation_id='stable-send'",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,bool>(1)?,row.get::<_,Option<String>>(2)?))).unwrap()
    };
    let before = snapshot();
    fixture
        .executor
        .dry_run(
            &actor(),
            &call("get_operation", json!({"operation_id":"stable-send"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(snapshot(), before);
    assert!(!before.1);
    assert!(before.2.is_some());
    let expired = fixture
        .executor
        .execute(
            &actor(),
            &call("get_operation", json!({"operation_id":"stable-send"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        expired.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(
        expired.operation().unwrap().result_availability(),
        tg_agent::journal::ResultAvailability::Expired
    );
    assert!(expired.operation().unwrap().message().is_none());
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
}

#[tokio::test]
async fn raw_update_and_primary_ids_preserve_extreme_precision_without_forwarded_scope_confusion() {
    let fixture = Fixture::new([]).await;
    let mut event =
        serde_json::to_value(message(i64::MAX, 1, "allowed forwarded content")).unwrap();
    event["message"]["message_id"] = json!(i64::MAX);
    event["message"]["forward_origin"] =
        json!({"type":"chat","date":1,"sender_chat":{"id":-9007199254740993i64,"type":"channel"}});
    fixture
        .append(vec![serde_json::from_value(event).unwrap()])
        .await;
    let result = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert_eq!(
        result.updates().unwrap().events()[0].chat_id(),
        Some(ChatId::new(1).unwrap())
    );
    let value = serde_json::to_value(&result).unwrap();
    let item = &value["updates"]["events"][0];
    assert_eq!(item["update_id"], i64::MAX.to_string());
    assert_eq!(item["message_id"], i64::MAX.to_string());
    let original: Value = serde_json::from_str(item["telegram_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        original["message"]["forward_origin"]["sender_chat"]["id"].as_i64(),
        Some(-9007199254740993)
    );
    assert!(!format!("{result:?}").contains("9007199254740993"));
}

#[tokio::test]
async fn mixed_inline_callbacks_require_unscoped_permission_and_never_issue_action_metadata() {
    let fixture = Fixture::new([]).await;
    let event = json!({"update_id":1,"callback_query":{"id":"PRIVATE_REFERENCE","from":{"id":5,"is_bot":false,"first_name":"Offline"},"chat_instance":"opaque","message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}},"inline_message_id":"PRIVATE_INLINE","data":"PRIVATE_DATA"}});
    fixture
        .append(vec![serde_json::from_value(event).unwrap()])
        .await;
    let scoped = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert!(scoped.updates().unwrap().events().is_empty());
    fixture
        .policy
        .replace(&actor(), "primary", grant().unscoped_updates(true))
        .unwrap();
    let explicit = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert_eq!(explicit.updates().unwrap().events().len(), 1);
    assert!(
        explicit.updates().unwrap().events()[0]
            .callback_query_id()
            .is_none()
    );
}

#[tokio::test]
async fn unscoped_permission_does_not_override_a_known_forbidden_chat_inside_an_extended_event() {
    let fixture = Fixture::new([]).await;
    let mut event = serde_json::to_value(message(1, 2, "PRIVATE_OTHER_CHAT")).unwrap();
    event["future_metadata"] = json!({"extra":true});
    fixture
        .append(vec![
            serde_json::from_value(event).unwrap(),
            message(2, 0, "PRIVATE_INVALID_CHAT"),
        ])
        .await;
    fixture
        .policy
        .replace(&actor(), "primary", grant().unscoped_updates(true))
        .unwrap();
    let result = fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    assert!(result.updates().unwrap().events().is_empty());
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("PRIVATE_OTHER_CHAT")
    );
}

#[tokio::test]
async fn reaction_message_and_generation_thread_ids_have_lossless_action_projections() {
    let fixture = Fixture::new([]).await;
    fixture.append(vec![
        serde_json::from_value(json!({"update_id":1,"message_reaction":{"chat":{"id":1,"type":"private"},"message_id":9007199254740993i64,"date":1,"old_reaction":[],"new_reaction":[]}})).unwrap(),
        serde_json::from_value(json!({"update_id":2,"message_reaction_count":{"chat":{"id":1,"type":"private"},"message_id":9007199254740993i64,"date":1,"reactions":[]}})).unwrap(),
        serde_json::from_value(json!({"update_id":3,"stopped_message_generation":{"chat":{"id":1,"type":"private"},"message_thread_id":9007199254740993i64,"draft_id":1}})).unwrap(),
    ]).await;
    let value = serde_json::to_value(
        fixture
            .executor
            .execute(&actor(), &call("read_updates", json!({})), None)
            .await
            .unwrap(),
    )
    .unwrap();
    let events = value["updates"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["message_id"], "9007199254740993");
    assert_eq!(events[1]["message_id"], "9007199254740993");
    assert_eq!(events[2]["message_thread_id"], "9007199254740993");
}

#[tokio::test]
async fn read_invocation_and_cost_caps_survive_restart_before_any_new_identity_rpc() {
    use std::time::Duration;
    use tg_agent::executor::BudgetLimits;
    let fixture = Fixture::new([identity()]).await;
    let budget = Budget::new("durable-read-budget")
        .unwrap()
        .window(Duration::from_secs(365 * 24 * 3600))
        .limits(BudgetLimits {
            invocations: 2,
            cost_units: 3,
            ..BudgetLimits::default()
        });
    let grants = Grant::new(0, budget)
        .allow(Action::GetMe)
        .allow(Action::ReadUpdates)
        .action_cost(Action::GetMe, 2);
    fixture.policy.replace(&actor(), "primary", grants).unwrap();
    fixture
        .executor
        .execute(&actor(), &call("read_updates", json!({})), None)
        .await
        .unwrap();
    fixture
        .executor
        .execute(&actor(), &call("get_me", json!({})), None)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .executor
            .execute(&actor(), &call("get_me", json!({})), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    let path = fixture._directory.path().join("agent.sqlite");
    let row: (i64, i64, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT invocations,write_attempts,cost_units FROM executor_budgets",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (2, 0, 3));
    fixture.journal.close().await.unwrap();
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let no_network = MockTransport::new([]).unwrap();
    let executor = Executor::new(fixture.policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(reopened, "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            no_network.bot().unwrap(),
        )
        .unwrap();
    assert_eq!(
        executor
            .execute(&actor(), &call("get_me", json!({})), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert!(no_network.requests().unwrap().is_empty());
}

#[tokio::test]
async fn revocation_during_remote_chat_read_prevents_result_disclosure() {
    use std::time::Duration;
    let info = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(1),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    )
    .title("PRIVATE_LATE_TITLE".into());
    let fixture = Fixture::new([
        identity(),
        Step::reply(
            "getChat",
            MockResponse::json(json!({"ok":true,"result":info})).delay(Duration::from_millis(200)),
        ),
    ])
    .await;
    let worker = fixture.executor.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(&actor(), &call("get_chat", json!({"chat_id":"1"})), None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !fixture
            .mock
            .requests()
            .unwrap()
            .iter()
            .any(|v| v.method() == "getChat")
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    fixture.policy.revoke(&actor(), "primary").unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::Denied);
    assert!(!format!("{error:?} {error}").contains("PRIVATE_LATE_TITLE"));
}
