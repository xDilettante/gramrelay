//! Offline durable intake followed by a separately acknowledged consumer.
use serde_json::json;
use std::time::Duration;
use tg::{
    CancellationToken, Router,
    test_kit::{MockResponse, MockTransport, Step},
};
use tg_agent::{
    journal::{Journal, JournalOptions},
    runtime::{ConsumerOptions, PollOptions},
    updates::{UpdateFeed, UpdateOptions},
};

fn reply(method: &'static str, result: serde_json::Value) -> Step {
    Step::reply(
        method,
        MockResponse::json(json!({"ok":true,"result":result})),
    )
}
fn identity() -> Step {
    reply(
        "getMe",
        json!({"id":0,"is_bot":true,"first_name":"Offline","username":"offline_bot"}),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed = UpdateFeed::open(journal.clone(), "offline", 0, UpdateOptions::default()).await?;
    let mock = MockTransport::new([
        identity(),
        reply(
            "getWebhookInfo",
            json!({"url":"","has_custom_certificate":false,"pending_update_count":0}),
        ),
        reply(
            "getUpdates",
            json!([
                {"update_id":10,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"one"}},
                {"update_id":11,"message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"two"}}
            ]),
        ),
        Step::reply(
            "getUpdates",
            MockResponse::json(json!({"ok":true,"result":[]})).delay(Duration::from_secs(30)),
        ),
        identity(),
    ])?;
    let bot = mock.bot()?;
    let stop = CancellationToken::new();
    let collector = {
        let feed = feed.clone();
        let bot = bot.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            feed.run_polling_until(&bot, PollOptions::default().poll_timeout(0), stop)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while mock.requests().unwrap().len() < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    stop.cancel();
    collector.await??;
    assert_eq!(
        mock.requests()?[3]
            .decode::<tg::api::methods::GetUpdates>()?
            .offset,
        Some(12)
    );
    assert_eq!(feed.read(None, 100).await?.events().len(), 2);

    let stop = CancellationToken::new();
    let router = Router::with_state(stop.clone()).on_message(|ctx| async move {
        // Real effects here need stable operation IDs in the operation journal.
        ctx.state().cancel();
        Ok(())
    });
    feed.run_consumer_until(&bot, "router", router, ConsumerOptions::default(), stop)
        .await?;
    let consumer = feed.claim_consumer("router").await?;
    assert!(
        feed.read_consumer(&consumer, 100)
            .await?
            .events()
            .is_empty()
    );
    feed.release_consumer(consumer).await?;
    mock.assert_finished()?;
    journal.close().await?;
    println!("Offline durable intake and consumer acknowledgement passed for two events.");
    Ok(())
}
