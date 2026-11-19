use serde_json::json;
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    updates::{SourceMode, UpdateCursor, UpdateFeed, UpdateOptions},
};

fn update(id: i64, text: &str) -> tg::Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"},"text":text,"future_message":{"value":1}},"future_metadata":{"value":2}})).unwrap()
}

#[tokio::test]
async fn durable_updates_deduplicate_and_page_without_erasing_unknown_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 123, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert!(
        feed.append_webhook(&source, update(10, "PRIVATE_TEXT"))
            .await
            .unwrap()
            .inserted()
    );
    assert!(
        !feed
            .append_webhook(&source, update(10, "PRIVATE_TEXT"))
            .await
            .unwrap()
            .inserted()
    );
    assert_eq!(
        feed.append_webhook(&source, update(10, "conflict"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    feed.append_webhook(&source, update(8, "arrived later"))
        .await
        .unwrap();
    let page = feed.read(None, 1).await.unwrap();
    assert_eq!(page.events().len(), 1);
    assert_eq!(page.events()[0].update().update_id, 10);
    assert_eq!(
        page.events()[0]
            .update()
            .unknown_fields()
            .get("future_metadata")
            .unwrap()["value"],
        2
    );
    assert!(!format!("{page:?} {source:?} {feed:?}").contains("PRIVATE_"));
    assert!(page.has_more());
    let cursor = page.next_cursor().clone();
    let serialized = serde_json::to_string(&cursor).unwrap();
    let cursor: UpdateCursor = serde_json::from_str(&serialized).unwrap();
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 123, UpdateOptions::default())
        .await
        .unwrap();
    let page = feed.read(Some(&cursor), 100).await.unwrap();
    assert_eq!(page.events().len(), 1);
    assert_eq!(page.events()[0].update().update_id, 8);
    assert!(!page.has_more());
    journal.close().await.unwrap();
}

#[tokio::test]
async fn binding_source_fences_and_cursors_cannot_cross_feeds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "first", 1, UpdateOptions::default())
        .await
        .unwrap();
    assert_eq!(
        UpdateFeed::open(journal.clone(), "alias", 1, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(
        UpdateFeed::open(journal.clone(), "first", 2, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(
        UpdateFeed::open(
            journal.clone(),
            "first",
            1,
            UpdateOptions::default().retention(std::time::Duration::from_secs(30))
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Conflict
    );
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        feed.claim_source(SourceMode::Polling)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    let other = UpdateFeed::open(journal.clone(), "second", 2, UpdateOptions::default())
        .await
        .unwrap();
    assert_eq!(
        other
            .append_webhook(&source, update(1, "wrong scope"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    feed.append_webhook(&source, update(1, "first"))
        .await
        .unwrap();
    let cursor = feed.read(None, 100).await.unwrap().next_cursor().clone();
    assert_eq!(
        other.read(Some(&cursor), 100).await.unwrap_err().kind(),
        JournalErrorKind::Denied
    );
    feed.release_source(source.clone()).await.unwrap();
    let next = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        feed.append_webhook(&source, update(2, "stale"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    feed.append_webhook(&next, update(2, "new owner"))
        .await
        .unwrap();
    assert!("garbage".parse::<UpdateCursor>().is_err());
    assert!("x".repeat(513).parse::<UpdateCursor>().is_err());
    journal.close().await.unwrap();
}

#[tokio::test]
async fn global_entry_byte_and_feed_caps_are_atomic_and_expiry_is_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(
        &path,
        JournalOptions::default()
            .max_update_entries(2)
            .max_update_feeds(2),
    )
    .await
    .unwrap();
    let first = UpdateFeed::open(journal.clone(), "one", 1, UpdateOptions::default())
        .await
        .unwrap();
    let second = UpdateFeed::open(journal.clone(), "two", 2, UpdateOptions::default())
        .await
        .unwrap();
    assert_eq!(
        UpdateFeed::open(journal.clone(), "three", 3, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    let a = first.claim_source(SourceMode::Webhook).await.unwrap();
    let b = second.claim_source(SourceMode::Webhook).await.unwrap();
    first.append_webhook(&a, update(1, "a")).await.unwrap();
    let old = first.read(None, 100).await.unwrap().next_cursor().clone();
    second.append_webhook(&b, update(2, "b")).await.unwrap();
    assert_eq!(
        first
            .append_webhook(&a, update(3, "full"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE updates SET expires_at_ms=0 WHERE profile='one'", [])
        .unwrap();
    assert_eq!(journal.prune_updates(100).await.unwrap(), 1);
    first
        .append_webhook(&a, update(3, "now fits"))
        .await
        .unwrap();
    // The last returned cursor is still a valid boundary; a cursor before expiry is not.
    assert_eq!(first.read(Some(&old), 100).await.unwrap().events().len(), 1);
    let saved = first.read(None, 100).await.unwrap().next_cursor().clone();
    connection
        .execute("UPDATE updates SET expires_at_ms=0 WHERE profile='one'", [])
        .unwrap();
    journal.prune_updates(100).await.unwrap();
    assert_eq!(
        first.read(Some(&old), 100).await.unwrap_err().kind(),
        JournalErrorKind::CursorExpired
    );
    assert!(
        first
            .read(Some(&saved), 100)
            .await
            .unwrap()
            .events()
            .is_empty()
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn utf8_payload_bytes_are_bounded_and_invalid_updates_never_enter_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_update_bytes(600))
        .await
        .unwrap();
    let feed = UpdateFeed::open(
        journal.clone(),
        "bot",
        1,
        UpdateOptions::default()
            .max_event_bytes(600)
            .max_batch_bytes(600),
    )
    .await
    .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        feed.append_webhook(&source, update(-1, "invalid"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    let mut ambiguous = update(99, "ambiguous");
    ambiguous.edited_message = ambiguous.message.clone();
    assert_eq!(
        feed.append_webhook(&source, ambiguous)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    feed.append_webhook(&source, update(1, &"😀".repeat(50)))
        .await
        .unwrap();
    assert_eq!(
        feed.append_webhook(&source, update(2, &"😀".repeat(50)))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 1);
    assert!(feed.read(None, 0).await.is_err());
    assert!(feed.read(None, 101).await.is_err());
    journal.close().await.unwrap();
}

#[tokio::test]
async fn source_expiry_and_close_release_only_the_current_workers_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let a = UpdateFeed::open(first.clone(), "a", 1, UpdateOptions::default())
        .await
        .unwrap();
    let b = UpdateFeed::open(second.clone(), "b", 2, UpdateOptions::default())
        .await
        .unwrap();
    let stale = a.claim_source(SourceMode::Webhook).await.unwrap();
    let other = b.claim_source(SourceMode::Webhook).await.unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE update_feeds SET source_until_ms=0 WHERE profile='a'",
            [],
        )
        .unwrap();
    assert_eq!(
        a.renew_source(&stale).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        a.append_webhook(&stale, update(1, "expired"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let current = a.claim_source(SourceMode::Webhook).await.unwrap();
    first.close().await.unwrap();
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let a = UpdateFeed::open(reopened.clone(), "a", 1, UpdateOptions::default())
        .await
        .unwrap();
    let next = a.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        a.append_webhook(&current, update(1, "old owner"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    a.append_webhook(&next, update(1, "new owner"))
        .await
        .unwrap();
    b.append_webhook(&other, update(1, "other still owns"))
        .await
        .unwrap();
    second.close().await.unwrap();
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn damaged_payloads_fail_without_a_new_cursor_or_private_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source, update(1, "PRIVATE_CONTENT"))
        .await
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE updates SET payload=x'505249564154455f434f5252555054'",
            [],
        )
        .unwrap();
    let error = feed.read(None, 100).await.unwrap_err();
    assert_eq!(error.kind(), JournalErrorKind::Storage);
    assert!(!format!("{error:?} {error}").contains("PRIVATE_"));
    journal.close().await.unwrap();
}

#[tokio::test]
async fn source_leases_are_bound_to_the_issuing_database_instance() {
    let directory = tempfile::tempdir().unwrap();
    let first = Journal::open(directory.path().join("a.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(directory.path().join("b.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let a = UpdateFeed::open(first.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let b = UpdateFeed::open(second.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let a_lease = a.claim_source(SourceMode::Webhook).await.unwrap();
    let b_lease = b.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        b.append_webhook(&a_lease, update(1, "wrong issuer"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    assert_eq!(
        b.release_source(a_lease).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    b.append_webhook(&b_lease, update(1, "right issuer"))
        .await
        .unwrap();
    first.close().await.unwrap();
    second.close().await.unwrap();
}
