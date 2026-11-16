use serde_json::json;
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    cli::serve_jsonl,
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore, Principal,
    },
    journal::{Journal, JournalOptions},
    tools::{ExecutionMode, OutputLimit, ToolSession},
    updates::{UpdateFeed, UpdateOptions},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let policy = PolicyStore::default();
    let principal = Principal::new("trusted-host")?;
    policy.replace(
        &principal,
        "demo",
        Grant::new(0, Budget::new("demo-budget")?)
            .allow(Action::SendMessage)
            .chats(ChatScope::only([ChatId::new(1)?])),
    )?;
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(
                json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
            ),
        ),
        Step::reply(
            "sendMessage",
            MockResponse::json(
                json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}),
            ),
        ),
    ])?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(
        UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default()).await?,
        mock.bot()?,
    )?;
    let session = ToolSession::new(executor, principal);
    let request = json!({"schema_version":1,"profile":"demo","operation_id":"stable-key","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"Hello"}}});
    let input = format!("{request}\n{request}\n");
    let mut output = Vec::new();
    serve_jsonl(
        &session,
        input.as_bytes(),
        &mut output,
        ExecutionMode::Execute,
        OutputLimit::default(),
    )
    .await?;
    assert_eq!(mock.requests()?.len(), 2);
    mock.assert_finished()?;
    journal.close().await?;
    print!("{}", String::from_utf8(output)?);
    Ok(())
}
