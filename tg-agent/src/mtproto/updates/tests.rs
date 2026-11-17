use super::*;
use project::Stamp;
async fn setup(journal: Journal) -> Feed {
    Feed::open(
        journal,
        "synthetic",
        ProfileIdentity::mtproto_user(42).unwrap(),
        Options::default(),
    )
    .await
    .unwrap()
}
fn stamp(ordinal: u64) -> Stamp {
    Stamp {
        source: [1; 32],
        id: [2; 32],
        kind: "events",
        ordinal,
    }
}
fn event(feed: &Feed) -> project::Encoded {
    project::encode(Event::Checkpoint, None, &feed.options).unwrap()
}

#[tokio::test]
async fn watermark_keeps_full_u64_range_and_rejects_content_source_or_order_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("watermark.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = setup(journal.clone()).await;
    let lease = feed.claim_source().await.unwrap();
    feed.append(&lease, stamp(u64::MAX), vec![event(&feed)], Instant::now())
        .await
        .unwrap();
    assert!(
        !feed
            .append(&lease, stamp(u64::MAX), vec![event(&feed)], Instant::now())
            .await
            .unwrap()
            .inserted()
    );
    assert_eq!(
        feed.append(
            &lease,
            stamp(u64::MAX - 1),
            vec![event(&feed)],
            Instant::now()
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Conflict
    );
    let mut foreign = stamp(u64::MAX);
    foreign.source = [9; 32];
    assert_eq!(
        feed.append(&lease, foreign, vec![event(&feed)], Instant::now())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    let altered = project::encode(Event::Baseline, None, &feed.options).unwrap();
    assert_eq!(
        feed.append(&lease, stamp(u64::MAX), vec![altered], Instant::now())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(
        feed.read_with_budget(None, 10, 1).await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    assert_eq!(feed.read(None, 10).await.unwrap().events().len(), 1);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute("UPDATE mtproto_updates SET payload_hash=zeroblob(32)", [])
        .unwrap();
    drop(c);
    assert_eq!(
        feed.read(None, 10).await.unwrap_err().kind(),
        JournalErrorKind::Storage
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn source_fencing_distinguishes_workers_expiry_and_foreign_close() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let a = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let b = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let fa = setup(a.clone()).await;
    let fb = setup(b.clone()).await;
    let old = fa.claim_source().await.unwrap();
    assert_eq!(
        fb.claim_source().await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    assert_eq!(
        fb.append(&old, stamp(1), vec![event(&fb)], Instant::now())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    a.run(|c, _, _| {
        c.execute("UPDATE mtproto_update_feeds SET source_until_ms=0", [])
            .map_err(|_| storage())?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        fa.append(&old, stamp(1), vec![event(&fa)], Instant::now())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let current = fb.claim_source().await.unwrap();
    assert_eq!(
        fa.renew_source(&old).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    a.close().await.unwrap();
    assert_eq!(
        fb.claim_source().await.unwrap_err().kind(),
        JournalErrorKind::Capacity,
        "closing A must not release B's source"
    );
    fb.append(&current, stamp(1), vec![event(&fb)], Instant::now())
        .await
        .unwrap();
    b.close().await.unwrap();
    let c = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let fc = setup(c.clone()).await;
    fc.claim_source().await.unwrap();
    c.close().await.unwrap();
}

#[tokio::test]
async fn global_feed_capacity_rolls_back_the_new_account_binding() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("feeds.sqlite"),
        JournalOptions::default().max_update_feeds(1),
    )
    .await
    .unwrap();
    setup(journal.clone()).await;
    assert_eq!(
        crate::updates::UpdateFeed::open(journal.clone(), "bot", 1, Options::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert!(journal.profile_identity("bot").await.unwrap().is_none());
    assert_eq!(
        Feed::open(
            journal.clone(),
            "other",
            ProfileIdentity::mtproto_bot(99).unwrap(),
            Options::default()
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Capacity
    );
    assert!(journal.profile_identity("other").await.unwrap().is_none());
    journal.close().await.unwrap();
}

#[tokio::test]
async fn global_byte_budget_counts_bot_payload_and_private_callback_bytes() {
    use crate::updates::{SourceMode, UpdateFeed};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bytes.sqlite");
    let callback = || Event::Callback {
        peer: Some(PeerId::new(PeerKind::User, 7).unwrap()),
        message_id: Some(crate::executor::MessageId::new(1).unwrap()),
        actor: PeerId::new(PeerKind::User, 99).unwrap(),
        data_base64: Some("ZGF0YQ==".into()),
        game: None,
    };
    let encoded = project::encode(callback(), Some(0), &Options::default()).unwrap();
    let mt_bytes = encoded.payload.len() + 8;
    let bot_event: tg::Update = serde_json::from_value(serde_json::json!({"update_id":1})).unwrap();
    let bot_bytes = serde_json::to_vec(&bot_event).unwrap().len();
    let total = mt_bytes + bot_bytes;
    let opts = Options::default()
        .max_event_bytes(mt_bytes)
        .max_batch_bytes(mt_bytes);
    let journal = Journal::open(&path, JournalOptions::default().max_update_bytes(total - 1))
        .await
        .unwrap();
    let bot = UpdateFeed::open(
        journal.clone(),
        "bot",
        1,
        Options::default()
            .max_event_bytes(bot_bytes)
            .max_batch_bytes(bot_bytes),
    )
    .await
    .unwrap();
    let source = bot.claim_source(SourceMode::Webhook).await.unwrap();
    bot.append_webhook(&source, bot_event).await.unwrap();
    let account = ProfileIdentity::mtproto_bot(42).unwrap();
    let feed = Feed::open(journal.clone(), "synthetic", account, opts.clone())
        .await
        .unwrap();
    let source = feed.claim_source().await.unwrap();
    assert_eq!(
        feed.append(&source, stamp(1), vec![encoded], Instant::now())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert!(feed.read(None, 10).await.unwrap().events().is_empty());
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default().max_update_bytes(total))
        .await
        .unwrap();
    let feed = Feed::open(journal.clone(), "synthetic", account, opts)
        .await
        .unwrap();
    let source = feed.claim_source().await.unwrap();
    let encoded = project::encode(callback(), Some(0), &feed.options).unwrap();
    feed.append(&source, stamp(1), vec![encoded], Instant::now())
        .await
        .unwrap();
    assert_eq!(
        journal
            .run(|c, _, _| crate::updates::storage_usage(c))
            .await
            .unwrap(),
        (2, total as i64)
    );
    journal.close().await.unwrap();
}

#[test]
fn projection_is_explicit_for_unscoped_unknowns_and_rejects_malformed_or_unbounded_known_data() {
    use tg_mtproto::tl::{self, Identifiable};
    let hidden = tl::enums::Update::UserName(tl::types::UpdateUserName {
        user_id: 7,
        first_name: "PRIVATE_FIRST".into(),
        last_name: "PRIVATE_LAST".into(),
        usernames: vec![],
    });
    let (event, query) = project::update(&hidden, 1024).unwrap();
    assert!(event.peer().is_none());
    assert!(query.is_none());
    assert!(
        matches!(event,Event::Unsupported {constructor} if constructor==tl::types::UpdateUserName::CONSTRUCTOR_ID)
    );
    assert!(!serde_json::to_string(&event).unwrap().contains("PRIVATE_"));
    let unscoped = tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
        messages: vec![7],
        pts: 1,
        pts_count: 1,
    });
    assert!(project::update(&unscoped, 1024).unwrap().0.peer().is_none());
    let huge = tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
        messages: vec![1; 4097],
        pts: 1,
        pts_count: 1,
    });
    assert_eq!(
        project::update(&huge, 1024).unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    let malformed = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: tl::enums::Message::Empty(tl::types::MessageEmpty {
            id: 0,
            peer_id: None,
        }),
        pts: 1,
        pts_count: 1,
    });
    assert_eq!(
        project::update(&malformed, 1024).unwrap_err().kind(),
        JournalErrorKind::Validation
    );
}
