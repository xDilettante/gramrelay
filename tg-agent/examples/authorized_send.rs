use serde_json::json;
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore,
        Principal, Request,
    },
    journal::{Journal, JournalOptions, OperationState},
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
                json!({"ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"},"text":"not stored in the receipt"}}),
            ),
        ),
    ])?;
    // The host supplies identity, capabilities and an immutable bot profile.
    let actor = Principal::new("trusted-host")?;
    let policy = PolicyStore::default();
    policy.replace(
        &actor,
        "demo",
        Grant::new(0, Budget::new("persistent-budget")?)
            .allow(Action::SendMessage)
            .chats(ChatScope::only([ChatId::new(1)?])),
    )?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(
        UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default()).await?,
        mock.bot()?,
    )?;
    // The same bounded versioned input will be used by CLI/MCP adapters.
    let request = Request::from_json(br#"{"schema_version":1,"profile":"demo","operation_id":"stable-example-key","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"Offline authorized example"}}}"#)?;
    for _ in 0..2 {
        let result = executor.execute(&actor, &request, None).await?;
        assert_eq!(
            result.operation().unwrap().state(),
            OperationState::Succeeded
        );
        println!("{}", serde_json::to_string(&result)?);
    }
    assert_eq!(mock.requests()?.len(), 2); // One identity read, one send.
    mock.assert_finished()?;
    journal.close().await?;
    println!("Two authorized invocations, one send; no sockets or real credentials.");
    Ok(())
}
