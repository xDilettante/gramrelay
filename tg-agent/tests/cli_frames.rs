#![cfg(feature = "cli")]
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    cli::{run_json, serve_jsonl},
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore, Principal,
    },
    journal::{Journal, JournalOptions, OperationState},
    tools::{ExecutionMode, OutputLimit, ToolSession},
    updates::{UpdateFeed, UpdateOptions},
};
fn identity() -> Step {
    Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ),
    )
}
fn sent() -> Step {
    Step::reply(
        "sendMessage",
        MockResponse::json(
            json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}),
        ),
    )
}
fn request() -> Value {
    json!({"schema_version":1,"profile":"demo","operation_id":"same-key","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"hello"}}})
}
struct Fixture {
    _directory: tempfile::TempDir,
    journal: Journal,
    mock: MockTransport,
    session: ToolSession,
}
impl Fixture {
    async fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("cli.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let principal = Principal::new("host").unwrap();
        let policy = PolicyStore::default();
        policy
            .replace(
                &principal,
                "demo",
                Grant::new(0, Budget::new("budget").unwrap())
                    .allow(Action::GetMe)
                    .allow(Action::GetChat)
                    .allow(Action::SendMessage)
                    .chats(ChatScope::only([ChatId::new(1).unwrap()])),
            )
            .unwrap();
        let mock = MockTransport::new(steps).unwrap();
        let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                mock.bot().unwrap(),
            )
            .unwrap();
        Self {
            _directory: directory,
            journal,
            mock,
            session: ToolSession::new(executor, principal),
        }
    }
}
#[tokio::test]
async fn bounded_bad_frame_continues_and_duplicate_writes_share_one_attempt() {
    let fixture = Fixture::new([identity(), sent()]).await;
    let request = request().to_string();
    let input = format!("{{\"principal\":\"PRIVATE_BAD\"}}\n{request}\n{request}\n");
    let mut output = Vec::new();
    serve_jsonl(
        &fixture.session,
        input.as_bytes(),
        &mut output,
        ExecutionMode::Execute,
        OutputLimit::default(),
    )
    .await
    .unwrap();
    let frames = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["ok"], false);
    assert!(!frames[0].to_string().contains("PRIVATE"));
    assert_eq!(frames[1]["result"]["operation"]["state"], "succeeded");
    assert_eq!(frames[2], frames[1]);
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
    fixture.mock.assert_finished().unwrap();
    fixture.journal.close().await.unwrap();
}
#[tokio::test]
async fn oversized_frame_stops_before_dispatch_instead_of_resynchronizing_into_an_action() {
    let fixture = Fixture::new([]).await;
    let mut input = vec![b'a'; 65537];
    input.push(b'\n');
    input.extend_from_slice(request().to_string().as_bytes());
    input.push(b'\n');
    let mut output = Vec::new();
    assert_eq!(
        serve_jsonl(
            &fixture.session,
            input.as_slice(),
            &mut output,
            ExecutionMode::Execute,
            OutputLimit::default()
        )
        .await
        .unwrap_err()
        .code(),
        "agent_input_too_large"
    );
    assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
    assert!(fixture.mock.requests().unwrap().is_empty());
    fixture.journal.close().await.unwrap();
}
#[tokio::test]
async fn single_document_rejects_trailing_json_and_dry_run_does_not_reserve_or_send() {
    let fixture = Fixture::new([]).await;
    let input = format!("{}\n{}", request(), request());
    let mut output = Vec::new();
    assert!(
        run_json(
            &fixture.session,
            input.as_bytes(),
            &mut output,
            ExecutionMode::Execute,
            OutputLimit::default()
        )
        .await
        .unwrap()
    );
    let mut output = Vec::new();
    assert!(
        !run_json(
            &fixture.session,
            request().to_string().as_bytes(),
            &mut output,
            ExecutionMode::DryRun,
            OutputLimit::default()
        )
        .await
        .unwrap()
    );
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["dry_run"]["is_write"], true);
    assert!(fixture.mock.requests().unwrap().is_empty());
    assert!(
        fixture
            .journal
            .get("demo", "host", "same-key")
            .await
            .unwrap()
            .is_none()
    );
    fixture.journal.close().await.unwrap();
}
struct Broken;
impl tokio::io::AsyncWrite for Broken {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn failed_output_after_a_write_preserves_the_receipt_and_does_not_reexecute() {
    let fixture = Fixture::new([identity(), sent()]).await;
    let error = run_json(
        &fixture.session,
        request().to_string().as_bytes(),
        Broken,
        ExecutionMode::Execute,
        OutputLimit::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "agent_io");
    let recovery = serde_json::to_value(&error).unwrap();
    assert_eq!(recovery["operation_id"], "same-key");
    assert_eq!(recovery["next_action"], "get_operation");
    assert_eq!(recovery["effect_outcome"], "unknown");
    assert_eq!(
        fixture
            .journal
            .get("demo", "host", "same-key")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    let mut output = Vec::new();
    run_json(
        &fixture.session,
        request().to_string().as_bytes(),
        &mut output,
        ExecutionMode::Execute,
        OutputLimit::default(),
    )
    .await
    .unwrap();
    assert_eq!(fixture.mock.requests().unwrap().len(), 2);
    fixture.mock.assert_finished().unwrap();
    fixture.journal.close().await.unwrap();
}
#[tokio::test]
async fn output_limit_emits_a_safe_error_and_keeps_the_next_frame_usable() {
    let chat = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(1),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    )
    .title("x".repeat(1024));
    let fixture = Fixture::new([
        identity(),
        Step::reply(
            "getChat",
            MockResponse::json(json!({"ok":true,"result":chat})),
        ),
    ])
    .await;
    let input = format!(
        "{}\n{}\n",
        json!({"schema_version":1,"profile":"demo","call":{"tool":"get_chat","arguments":{"chat_id":"1"}}}),
        json!({"schema_version":1,"profile":"demo","call":{"tool":"get_me","arguments":{}}})
    );
    let mut output = Vec::new();
    serve_jsonl(
        &fixture.session,
        input.as_bytes(),
        &mut output,
        ExecutionMode::Execute,
        OutputLimit::new(1024).unwrap(),
    )
    .await
    .unwrap();
    let frames = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["error"]["code"], "agent_output_too_large");
    assert!(frames[0]["error"].get("effect_outcome").is_none());
    assert_eq!(frames[1]["ok"], true);
    fixture.mock.assert_finished().unwrap();
    fixture.journal.close().await.unwrap();
}
