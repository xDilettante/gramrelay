use serde_json::json;
use tg_agent::{
    journal::{Journal, JournalOptions},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed =
        UpdateFeed::open(journal.clone(), "offline-demo", 1, UpdateOptions::default()).await?;
    let source = feed.claim_source(SourceMode::Webhook).await?;
    // Synthetic input. A real HTTP adapter must authenticate before calling this storage API.
    let update: tg::Update = serde_json::from_value(json!({"update_id":42,"message":{
        "message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"Offline durable update"
    }}))?;
    assert!(
        feed.append_webhook(&source, update.clone())
            .await?
            .inserted()
    );
    assert!(!feed.append_webhook(&source, update).await?.inserted());
    let page = feed.read(None, 100).await?;
    assert_eq!(page.events().len(), 1);
    assert!(
        feed.read(Some(page.next_cursor()), 100)
            .await?
            .events()
            .is_empty()
    );
    feed.release_source(source).await?;
    journal.close().await?;
    println!("Durable update, duplicate suppressed, cursor resumed; no network.");
    Ok(())
}
