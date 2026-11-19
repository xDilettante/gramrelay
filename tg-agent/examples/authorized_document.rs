use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
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
    let policy = PolicyStore::default();
    let principal = Principal::new("trusted-host")?;
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(
                json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
            ),
        ),
        Step::reply(
            "sendDocument",
            MockResponse::json(
                json!({"ok":true,"result":{"message_id":13,"date":1,"chat":{"id":1,"type":"private"}}}),
            ),
        ),
    ])?;
    let executor = Executor::new(policy.clone(), ExecutorOptions::default())?;
    executor.register_profile(
        UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default()).await?,
        mock.bot()?,
    )?;
    let expires = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64 + 60000;
    // Registration is host configuration; tool input receives only the handle.
    let file = executor
        .register_file(
            &principal,
            "demo",
            "report.txt",
            b"Offline report\n".to_vec(),
            expires,
        )
        .await?;
    policy.replace(
        &principal,
        "demo",
        Grant::new(0, Budget::new("document-budget")?)
            .allow(Action::SendDocument)
            .allow(Action::GetOperation)
            .chats(ChatScope::only([ChatId::new(1)?]))
            .files([file.handle()]),
    )?;
    let request = Request::from_json(&serde_json::to_vec(&json!({
        "schema_version":1,"profile":"demo","operation_id":"report-1",
        "call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":file.handle(),"caption":"Report"}}
    }))?)?;
    executor.dry_run(&principal, &request, None).await?;
    let result = executor.execute(&principal, &request, None).await?;
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    println!("{}", serde_json::to_string(&result)?);
    executor.discard_file(&principal, "demo", file.handle())?;
    let cached = executor.execute(&principal, &request, None).await?;
    assert_eq!(
        cached.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(mock.requests()?.len(), 2);
    mock.assert_finished()?;
    journal.close().await?;
    println!("One identity read, one immutable upload; cached receipt needs no resident bytes.");
    Ok(())
}
