//! Localhost HTTP with synthetic identity, SQLite intake and an independent consumer.
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tg::{
    CancellationToken, Router,
    test_kit::{MockResponse, MockTransport, Step},
    webhook::WebhookSecret,
};
use tg_agent::{
    journal::{Journal, JournalOptions},
    runtime::ConsumerOptions,
    updates::{UpdateFeed, UpdateOptions},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed = UpdateFeed::open(journal.clone(), "offline", 0, UpdateOptions::default()).await?;
    let mock = MockTransport::new((0..2).map(|_| {
        Step::reply(
            "getMe",
            MockResponse::json(
                json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
            ),
        )
    }))?;
    let bot = mock.bot()?;
    let secret = WebhookSecret::new("SYNTHETIC_LOCAL_SECRET")?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/telegram", listener.local_addr()?);
    let stop = CancellationToken::new();
    let server = {
        let feed = feed.clone();
        let bot = bot.clone();
        let secret = secret.clone();
        let stop = stop.clone();
        tokio::spawn(async move { feed.webhook(&bot, secret).serve_until(listener, stop).await })
    };
    let client = reqwest::Client::builder().no_proxy().build()?;
    for _ in 0..2 {
        let response=client.post(&url).header("x-telegram-bot-api-secret-token",secret.expose()).json(&json!({"update_id":1,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"offline"}})).send().await?;
        assert_eq!(response.status(), 200);
    }
    stop.cancel();
    server.await??;
    assert_eq!(feed.read(None, 100).await?.events().len(), 1);
    let processed = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let router =
        Router::with_state((processed.clone(), stop.clone())).on_message(|ctx| async move {
            // External effects here need stable operation IDs in the operation journal.
            ctx.state().0.fetch_add(1, Ordering::SeqCst);
            ctx.state().1.cancel();
            Ok(())
        });
    feed.run_consumer_until(&bot, "router", router, ConsumerOptions::default(), stop)
        .await?;
    assert_eq!(processed.load(Ordering::SeqCst), 1);
    mock.assert_finished()?;
    journal.close().await?;
    println!(
        "Offline durable webhook stored one update and acknowledged its independent consumer."
    );
    Ok(())
}
