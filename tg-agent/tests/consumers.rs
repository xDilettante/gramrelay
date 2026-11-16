use serde_json::json;
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};
fn update(id: i64) -> tg::Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"}}})).unwrap()
}

#[tokio::test]
async fn consumer_proofs_cannot_cross_database_instances() {
    let directory = tempfile::tempdir().unwrap();
    let mut feeds = Vec::new();
    let mut journals = Vec::new();
    for name in ["a.sqlite", "b.sqlite"] {
        let journal = Journal::open(directory.path().join(name), JournalOptions::default())
            .await
            .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
        feed.append_webhook(&source, update(1)).await.unwrap();
        journals.push(journal);
        feeds.push(feed);
    }
    let first = feeds[0].claim_consumer("same").await.unwrap();
    let second = feeds[1].claim_consumer("same").await.unwrap();
    let (_, ack) = feeds[0]
        .read_consumer(&first, 100)
        .await
        .unwrap()
        .into_parts();
    assert_eq!(
        feeds[1]
            .read_consumer(&first, 100)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feeds[1].renew_consumer(&first).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feeds[1].release_consumer(first).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feeds[1]
            .acknowledge_consumer(&second, ack)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feeds[1]
            .read_consumer(&second, 100)
            .await
            .unwrap()
            .events()
            .len(),
        1
    );
    for journal in journals {
        journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn reading_does_not_ack_and_committed_acknowledgements_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    for id in [20, 10] {
        feed.append_webhook(&source, update(id)).await.unwrap();
    }
    let lease = feed.claim_consumer("router").await.unwrap();
    let batch = feed.read_consumer(&lease, 1).await.unwrap();
    assert_eq!(batch.events()[0].update().update_id, 20);
    assert_eq!(
        feed.read_consumer(&lease, 100)
            .await
            .unwrap()
            .events()
            .len(),
        2
    );
    let (_, ack) = batch.into_parts();
    let stale = ack.clone();
    let lease = feed.renew_consumer(&lease).await.unwrap();
    feed.acknowledge_consumer(&lease, ack).await.unwrap();
    assert_eq!(
        feed.acknowledge_consumer(&lease, stale)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(
        feed.read_consumer(&lease, 100).await.unwrap().events()[0]
            .update()
            .update_id,
        10
    );
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let lease = feed.claim_consumer("router").await.unwrap();
    let batch = feed.read_consumer(&lease, 100).await.unwrap();
    assert_eq!(batch.events().len(), 1);
    let (_, ack) = batch.into_parts();
    let old = lease.clone();
    let old_ack = ack.clone();
    feed.release_consumer(lease).await.unwrap();
    let lease = feed.claim_consumer("router").await.unwrap();
    assert_eq!(
        feed.acknowledge_consumer(&old, old_ack)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feed.acknowledge_consumer(&lease, ack)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let (_, ack) = feed.read_consumer(&lease, 100).await.unwrap().into_parts();
    feed.acknowledge_consumer(&lease, ack).await.unwrap();
    assert!(
        feed.read_consumer(&lease, 100)
            .await
            .unwrap()
            .events()
            .is_empty()
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn independent_consumers_and_workers_are_fenced_and_unacknowledged_work_replays() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let a = UpdateFeed::open(first.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let b = UpdateFeed::open(second.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let source = a.claim_source(SourceMode::Webhook).await.unwrap();
    a.append_webhook(&source, update(1)).await.unwrap();
    let old = a.claim_consumer("shared").await.unwrap();
    let (_, old_ack) = a.read_consumer(&old, 100).await.unwrap().into_parts();
    assert_eq!(
        b.claim_consumer("shared").await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    let independent = b.claim_consumer("other").await.unwrap();
    let (_, ack) = b
        .read_consumer(&independent, 100)
        .await
        .unwrap()
        .into_parts();
    b.acknowledge_consumer(&independent, ack).await.unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE update_consumers SET lease_until_ms=0 WHERE consumer_id='shared'",
            [],
        )
        .unwrap();
    assert_eq!(
        a.renew_consumer(&old).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    let current = b.claim_consumer("shared").await.unwrap();
    assert_eq!(
        b.read_consumer(&current, 100).await.unwrap().events().len(),
        1
    );
    assert_eq!(
        a.acknowledge_consumer(&old, old_ack)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    first.close().await.unwrap();
    assert_eq!(
        b.read_consumer(&current, 100).await.unwrap().events().len(),
        1
    );
    second.close().await.unwrap();
}

#[tokio::test]
async fn retention_gaps_require_explicit_skip_and_cannot_acknowledge_another_consumer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_update_consumers(2))
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let a = feed.claim_consumer("a").await.unwrap();
    let b = feed.claim_consumer("b").await.unwrap();
    assert_eq!(
        feed.claim_consumer("full").await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source, update(1)).await.unwrap();
    let (_, ack) = feed.read_consumer(&a, 100).await.unwrap().into_parts();
    assert_eq!(
        feed.acknowledge_consumer(&b, ack).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE updates SET expires_at_ms=0", [])
        .unwrap();
    journal.prune_updates(100).await.unwrap();
    assert_eq!(
        feed.read_consumer(&a, 100).await.unwrap_err().kind(),
        JournalErrorKind::CursorExpired
    );
    feed.skip_expired_consumer(&a).await.unwrap();
    assert!(
        feed.read_consumer(&a, 100)
            .await
            .unwrap()
            .events()
            .is_empty()
    );
    feed.release_consumer(a.clone()).await.unwrap();
    assert_eq!(
        feed.read_consumer(&a, 100).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    journal.close().await.unwrap();
}
