#![cfg(feature = "executor")]
use serde_json::{Value, json};
use tg::test_kit::{MockResponse, MockTransport, Step};
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
fn call(tool: &str, key: Option<&str>, args: Value) -> Request {
    let mut value =
        json!({"schema_version":1,"profile":"primary","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        value["operation_id"] = json!(key);
    }
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn edit(text: &str) -> Request {
    call(
        "edit_message_text",
        Some("edit-key"),
        json!({"chat_id":"1","message_id":"9","text":text}),
    )
}
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ),
    )
}
async fn setup(journal: Journal, policy: PolicyStore, mock: &MockTransport) -> Executor {
    policy
        .replace(
            &actor(),
            "primary",
            Grant::new(0, Budget::new("quota").unwrap())
                .allow(Action::EditMessageText)
                .allow(Action::GetOperation)
                .chats(ChatScope::only([ChatId::new(1).unwrap()])),
        )
        .unwrap();
    let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal, "primary", 0, UpdateOptions::default())
                .await
                .unwrap(),
            mock.bot().unwrap(),
        )
        .unwrap();
    executor
}
#[tokio::test]
async fn a_normal_edit_confirms_exact_target_and_durably_reuses_one_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(),Step::reply("editMessageText",MockResponse::json(json!({"ok":true,"result":{"message_id":9,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_RESULT","future_private":"PRIVATE_FUTURE"}})))]).unwrap();
    let executor = setup(journal, PolicyStore::default(), &mock).await;
    for _ in 0..2 {
        let result = executor
            .execute(&actor(), &edit("PRIVATE_TEXT"), None)
            .await
            .unwrap();
        assert_eq!(
            result.operation().unwrap().state(),
            OperationState::Succeeded
        );
        assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE_"));
    }
    let recorded = mock.requests().unwrap();
    assert_eq!(recorded.len(), 2);
    let params = recorded[1]
        .decode::<tg::api::methods::EditMessageText>()
        .unwrap();
    assert_eq!(i64::from(params.message_id.unwrap()), 9);
    assert_eq!(params.text.as_deref(), Some("PRIVATE_TEXT"));
    assert!(params.inline_message_id.is_none());
    assert!(params.business_connection_id.is_none());
    assert!(params.parse_mode.is_none());
    assert_eq!(
        executor
            .execute(&actor(), &edit("changed"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
}
#[tokio::test]
async fn edit_wrong_chat_wrong_message_and_boolean_success_remain_unknown_without_replay() {
    for result in [
        json!({"message_id":9,"date":1,"chat":{"id":2,"type":"private"}}),
        json!({"message_id":10,"date":1,"chat":{"id":1,"type":"private"}}),
        json!(true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("agent.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let mock = MockTransport::new([
            identity(),
            Step::reply(
                "editMessageText",
                MockResponse::json(json!({"ok":true,"result":result})),
            ),
        ])
        .unwrap();
        let executor = setup(journal, PolicyStore::default(), &mock).await;
        for _ in 0..2 {
            assert_eq!(
                executor
                    .execute(&actor(), &edit("hello"), None)
                    .await
                    .unwrap()
                    .operation()
                    .unwrap()
                    .state(),
                OperationState::Unknown
            );
        }
        assert_eq!(mock.requests().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn edit_respects_current_chat_and_exact_host_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let mock = MockTransport::new([identity(),Step::reply("editMessageText",MockResponse::json(json!({"ok":true,"result":{"message_id":9,"date":1,"chat":{"id":1,"type":"private"}}})))]).unwrap();
    let policy = PolicyStore::default();
    let executor = setup(journal, policy.clone(), &mock).await;
    policy
        .replace(
            &actor(),
            "primary",
            Grant::new(0, Budget::new("quota").unwrap())
                .allow(Action::EditMessageText)
                .chats(ChatScope::only([ChatId::new(1).unwrap()]))
                .require_approval(Action::EditMessageText),
        )
        .unwrap();
    assert_eq!(
        executor
            .execute(&actor(), &edit("hello"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    assert!(mock.requests().unwrap().is_empty());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let approval = executor
        .approve(&actor(), &edit("hello"), now + 10000)
        .await
        .unwrap();
    assert!(
        executor
            .execute(&actor(), &edit("changed"), Some(&approval))
            .await
            .is_err()
    );
    assert_eq!(
        executor
            .execute(&actor(), &edit("hello"), Some(&approval))
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    policy.revoke(&actor(), "primary").unwrap();
    assert!(
        executor
            .execute(&actor(), &edit("hello"), Some(&approval))
            .await
            .is_err()
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
}
