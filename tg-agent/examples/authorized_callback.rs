use serde_json::json;
use tg::test_kit::{MockResponse, MockTransport, Step};
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
    // Synthetic authenticated host input. No Telegram connection is opened.
    let source = feed.claim_source(SourceMode::Webhook).await?;
    let event = serde_json::from_value(json!({
        "update_id":1,
        "callback_query":{
            "id":"demo-query", "from":{"id":1,"is_bot":false,"first_name":"Demo"},
            "chat_instance":"demo-instance", "data":"untrusted content",
            "message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}
        }
    }))?;
    feed.append_webhook(&source, event).await?;
    feed.release_source(source).await?;
    let principal = Principal::new("trusted-host")?;
    let policy = PolicyStore::default();
    policy.replace(
        &principal,
        "demo",
        Grant::new(0, Budget::new("callback-budget")?)
            .allow(Action::AnswerCallbackQuery)
            .allow(Action::GetOperation)
            .chats(ChatScope::only([ChatId::new(1)?])),
    )?;
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(json!({"ok":true,"result":{
                "id":0,"is_bot":true,"first_name":"Demo","username":"demo_bot"
            }})),
        ),
        Step::reply(
            "answerCallbackQuery",
            MockResponse::json(json!({"ok":true,"result":true})),
        ),
    ])?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(feed.clone(), mock.bot()?)?;
    let page = feed.read(None, 1).await?;
    let reference = executor.observe_callback(&principal, "demo", &page.events()[0])?;
    let request = Request::from_json(&serde_json::to_vec(&json!({
        "schema_version":1,"profile":"demo","operation_id":"ack-demo-1",
        "call":{"tool":"answer_callback_query","arguments":{
            "callback_query_id":reference.callback_query_id(),"text":"Accepted"
        }}
    }))?)?;
    executor.dry_run(&principal, &request, None).await?;
    for _ in 0..2 {
        let result = executor.execute(&principal, &request, None).await?;
        assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
        println!("{}", serde_json::to_string(&result)?);
    }
    assert_eq!(mock.requests()?.len(), 2);
    mock.assert_finished()?;
    journal.close().await?;
    println!("One identity read and one callback answer; repeated key reads the receipt.");
    Ok(())
}
