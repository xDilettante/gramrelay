use serde_json::json;
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};

fn update(id: i64) -> tg::Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_EVENT"}})).unwrap()
}

#[tokio::test]
async fn only_a_committed_whole_batch_advances_the_durable_offset() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_update_entries(2))
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let lease = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&lease).await.unwrap();
    assert_eq!(checkpoint.offset(), None);
    assert_eq!(
        feed.append_poll(
            &lease,
            &checkpoint,
            vec![update(10), update(20), update(30)]
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Capacity
    );
    assert!(feed.read(None, 100).await.unwrap().events().is_empty());
    assert_eq!(feed.poll_checkpoint(&lease).await.unwrap().offset(), None);
    let receipt = feed
        .append_poll(&lease, &checkpoint, vec![update(10), update(20)])
        .await
        .unwrap();
    assert_eq!(
        (
            receipt.inserted(),
            receipt.duplicates(),
            receipt.next_offset()
        ),
        (2, 0, Some(21))
    );
    assert!(!format!("{receipt:?} {checkpoint:?}").contains("PRIVATE_"));
    assert_eq!(
        feed.append_poll(&lease, &checkpoint, vec![update(30)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(
        feed.poll_checkpoint(&lease).await.unwrap().offset(),
        Some(21)
    );
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let lease = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(
        feed.poll_checkpoint(&lease).await.unwrap().offset(),
        Some(21)
    );
    assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 2);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn invalid_unsorted_and_conflicting_batches_leave_checkpoint_and_counters_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    for events in [
        vec![update(2), update(1)],
        vec![update(1), update(1)],
        vec![update(-1)],
        vec![update(i64::MAX)],
        (0..101).map(update).collect(),
    ] {
        assert_eq!(
            feed.append_poll(&source, &checkpoint, events)
                .await
                .unwrap_err()
                .kind(),
            JournalErrorKind::Validation
        );
    }
    feed.append_poll(&source, &checkpoint, vec![update(10)])
        .await
        .unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    assert_eq!(
        feed.append_poll(&source, &checkpoint, vec![update(9)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 1);
    let count: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT SUM(entry_count) FROM update_feeds", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn empty_polls_do_not_refresh_old_offsets_and_mode_changes_invalidate_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    feed.append_poll(&source, &checkpoint, vec![update(100)])
        .await
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE update_feeds SET poll_updated_at_ms=1", [])
        .unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    assert_eq!(checkpoint.offset(), None);
    feed.append_poll(&source, &checkpoint, vec![])
        .await
        .unwrap();
    assert_eq!(feed.poll_checkpoint(&source).await.unwrap().offset(), None);
    feed.append_poll(&source, &checkpoint, vec![update(5)])
        .await
        .unwrap();
    assert_eq!(
        feed.poll_checkpoint(&source).await.unwrap().offset(),
        Some(6)
    );
    feed.release_source(source).await.unwrap();
    let webhook = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        feed.poll_checkpoint(&webhook).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    feed.release_source(webhook).await.unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(feed.poll_checkpoint(&source).await.unwrap().offset(), None);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn stale_source_or_checkpoint_cannot_overwrite_a_new_owners_progress() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let old = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&old).await.unwrap();
    feed.release_source(old.clone()).await.unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(
        feed.append_poll(&old, &checkpoint, vec![update(1)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        feed.append_poll(&source, &checkpoint, vec![update(1)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let fresh = feed.poll_checkpoint(&source).await.unwrap();
    feed.append_poll(&source, &fresh, vec![update(2)])
        .await
        .unwrap();
    assert_eq!(
        feed.poll_checkpoint(&source).await.unwrap().offset(),
        Some(3)
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn payload_conflicts_and_batch_byte_limits_do_not_partially_store_new_events() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(
        journal.clone(),
        "bot",
        1,
        UpdateOptions::default()
            .max_event_bytes(256)
            .max_batch_bytes(256),
    )
    .await
    .unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    assert_eq!(
        feed.append_poll(&source, &checkpoint, vec![update(1), update(2), update(3)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert!(feed.read(None, 100).await.unwrap().events().is_empty());
    feed.append_poll(&source, &checkpoint, vec![update(10)])
        .await
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE update_feeds SET poll_updated_at_ms=1", [])
        .unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    let mut conflict = update(10);
    conflict.message.as_mut().unwrap().text = Some("changed".into());
    assert_eq!(
        feed.append_poll(&source, &checkpoint, vec![update(5), conflict])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 1);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn a_checkpoint_from_another_database_cannot_authorize_a_commit() {
    let directory = tempfile::tempdir().unwrap();
    let a = Journal::open(directory.path().join("a.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let b = Journal::open(directory.path().join("b.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let first = UpdateFeed::open(a.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let second = UpdateFeed::open(b.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let first_lease = first.claim_source(SourceMode::Polling).await.unwrap();
    let second_lease = second.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = first.poll_checkpoint(&first_lease).await.unwrap();
    assert_eq!(
        second
            .append_poll(&second_lease, &checkpoint, vec![update(1)])
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert!(second.read(None, 100).await.unwrap().events().is_empty());
    a.close().await.unwrap();
    b.close().await.unwrap();
}
