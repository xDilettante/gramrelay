use serde_json::json;
use tg::test_kit::MockTransport;
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore,
        Principal, Request,
    },
    journal::{Journal, JournalOptions},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default()).await?;
    // The trusted host's ingestion pipeline has already authenticated the source.
    // This example supplies synthetic events directly, without an HTTP server.
    let source = feed.claim_source(SourceMode::Webhook).await?;
    for (id, chat, text) in [
        (1, 2, "PRIVATE_OTHER_CHAT"),
        (2, 1, "Telegram content remains untrusted data"),
    ] {
        let event = serde_json::from_value(
            json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"},"text":text}}),
        )?;
        feed.append_webhook(&source, event).await?;
    }
    feed.release_source(source).await?;
    let principal = Principal::new("trusted-host")?;
    let policy = PolicyStore::default();
    policy.replace(
        &principal,
        "demo",
        Grant::new(0, Budget::new("persistent-read-budget")?)
            .allow(Action::ReadUpdates)
            .chats(ChatScope::only([ChatId::new(1)?])),
    )?;
    let mock = MockTransport::new([])?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(feed, mock.bot()?)?;
    let request = Request::from_json(br#"{"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{"limit":1}}}"#)?;
    executor.dry_run(&principal, &request, None).await?;
    let result = executor.execute(&principal, &request, None).await?;
    let page = result.updates().unwrap();
    assert_eq!(page.events().len(), 1);
    assert_eq!(page.events()[0].chat_id(), Some(ChatId::new(1)?));
    let output = serde_json::to_string(&result)?;
    assert!(!output.contains("PRIVATE_OTHER_CHAT"));
    println!("{output}");
    assert!(mock.requests()?.is_empty());
    mock.assert_finished()?;
    journal.close().await?;
    println!("One scoped local event; no getMe/getUpdates or other Telegram request.");
    Ok(())
}
