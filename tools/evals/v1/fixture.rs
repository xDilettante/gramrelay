#![allow(dead_code)]

use serde_json::{Value, json};
use tg::test_kit::{MockResponse, MockTransport, RecordedRequest, Step};

pub fn reply(method: &'static str, value: Value) -> Step {
    Step::reply(
        method,
        MockResponse::json(json!({"ok":true,"result":value})),
    )
}

pub fn message(method: &'static str, chat: i64, id: i64) -> Step {
    reply(
        method,
        json!({"message_id":id,"date":51,"chat":{"id":chat,"type":"private"}}),
    )
}

pub fn one(mock: &MockTransport, method: &str) -> RecordedRequest {
    mock.assert_finished().unwrap();
    let mut requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.method(), method);
    assert_eq!(request.http_method(), "POST");
    assert!(request.upload_complete());
    request
}

pub fn json_body(mock: &MockTransport, method: &str) -> Value {
    serde_json::from_slice(one(mock, method).body()).unwrap()
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|slice| slice == needle)
}

use tg_agent::{
    executor::{
        Action, ActionCall, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant,
        PolicyStore, Principal, Request, SendMessageArgs,
    },
    journal::{Journal, JournalOptions},
    updates::{UpdateFeed, UpdateOptions},
};
use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions, tl};

pub struct Host {
    pub executor: Executor,
    pub policy: PolicyStore,
    pub actor: Principal,
    pub journal: Journal,
    _directory: tempfile::TempDir,
}

pub async fn agent_fixture(mock: &MockTransport, profile: &str, actor: &str) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
    let feed = UpdateFeed::open(journal.clone(), profile, 0, UpdateOptions::default())
        .await
        .unwrap();
    let bot = mock
        .bot_builder()
        .timeout(std::time::Duration::from_millis(150))
        .build()
        .unwrap();
    executor.register_profile(feed, bot).unwrap();
    Host {
        executor,
        policy,
        actor: Principal::new(actor).unwrap(),
        journal,
        _directory: dir,
    }
}

pub fn identity() -> Step {
    reply(
        "getMe",
        json!({"id":0,"is_bot":true,"first_name":"Fixture","username":"fixture_bot"}),
    )
}

pub fn send(profile: &str, key: &str, chat: i64, text: &str) -> Request {
    Request::new(
        profile,
        Some(key.into()),
        ActionCall::SendMessage(SendMessageArgs {
            chat_id: ChatId::new(chat).unwrap(),
            text: text.into(),
            message_thread_id: None,
            disable_notification: None,
        }),
    )
    .unwrap()
}

pub fn allow<const N: usize>(
    host: &Host,
    profile: &str,
    chat: ChatId,
    actions: [Action; N],
    files: [&str; 0],
) {
    let mut grant = Grant::new(0, Budget::new("eval-scope").unwrap())
        .chats(ChatScope::only([chat]))
        .files(files);
    for action in actions {
        grant = grant.allow(action);
    }
    host.policy.replace(&host.actor, profile, grant).unwrap();
}

pub async fn mt_session() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::open(
        dir.path().join("mt.sqlite"),
        SessionBinding::new("local-fixture", 1234).unwrap(),
        SessionKey::from_bytes([31; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    (dir, session)
}

pub fn pong(wire: &[u8], expect: i64, message: i64) -> Vec<u8> {
    use tl::{Deserializable, Identifiable, Serializable};
    assert_eq!(
        &wire[..4],
        &tl::functions::Ping::CONSTRUCTOR_ID.to_le_bytes()
    );
    let request = tl::functions::Ping::from_bytes(&wire[4..]).unwrap();
    assert_eq!(request.ping_id, expect);
    tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: message,
        ping_id: request.ping_id,
    })
    .to_bytes()
}
