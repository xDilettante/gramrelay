#![cfg(feature = "mtproto-test-kit")]
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    mtproto::updates::{Cursor, Event, Feed, Ingested, Options},
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, ClientOptions, Session, SessionBinding, SessionKey,
    SessionOptions, UpdateItem, UpdateMode, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
fn expected(bot: bool) -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: if bot {
            AccountKind::Bot
        } else {
            AccountKind::User
        },
    }
}
fn account(bot: bool) -> ProfileIdentity {
    if bot {
        ProfileIdentity::mtproto_bot(42)
    } else {
        ProfileIdentity::mtproto_user(42)
    }
    .unwrap()
}
async fn session(path: &std::path::Path) -> Session {
    Session::open(
        path,
        SessionBinding::new("synthetic", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap()
}
fn user(id: i64, bot: bool, private: bool) -> tl::enums::User {
    let mut bytes = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    let flags = if id == 42 { 1u32 << 10 } else { 0 }
        | if bot { 1 << 14 } else { 0 }
        | if private { 1 | (1 << 1) | (1 << 4) } else { 0 };
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    if private {
        bytes.extend_from_slice(&777i64.to_le_bytes());
        bytes.extend_from_slice(&"PRIVATE_ENTITY".to_owned().to_bytes());
        bytes.extend_from_slice(&"PRIVATE_PHONE".to_owned().to_bytes());
    }
    if bot {
        bytes.extend_from_slice(&0i32.to_le_bytes());
    }
    tl::enums::User::from_bytes(&bytes).unwrap()
}
fn self_reply(bytes: &[u8], bot: bool) -> Option<Vec<u8>> {
    (bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes())
        .then(|| vec![user(42, bot, false)].to_bytes())
}
fn state(pts: i32) -> tl::enums::updates::State {
    tl::enums::updates::State::State(tl::types::updates::State {
        pts,
        qts: 0,
        seq: 0,
        date: 100 + pts,
        unread_count: 0,
    })
}
fn empty(pts: i32) -> Vec<u8> {
    tl::enums::updates::Difference::Empty(tl::types::updates::DifferenceEmpty {
        date: 100 + pts,
        seq: 0,
    })
    .to_bytes()
}
fn message(id: i32, text: &str) -> tl::enums::Message {
    let mut bytes = tl::types::Message::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(1u32 << 8).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 99 }).to_bytes());
    bytes.extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 7 }).to_bytes());
    bytes.extend_from_slice(&101i32.to_le_bytes());
    bytes.extend_from_slice(&text.to_owned().to_bytes());
    tl::enums::Message::from_bytes(&bytes).unwrap()
}
fn difference(
    messages: Vec<tl::enums::Message>,
    updates: Vec<tl::enums::Update>,
    pts: i32,
) -> Vec<u8> {
    tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
        new_messages: messages,
        new_encrypted_messages: vec![],
        other_updates: updates,
        chats: vec![],
        users: vec![user(7, false, true)],
        state: state(pts),
    })
    .to_bytes()
}
async fn batch(receiver: &mut tg_mtproto::UpdateReceiver) -> tg_mtproto::UpdateBatch {
    let Some(UpdateItem::Batch(batch)) = receiver.next_batch(Duration::ZERO).await.unwrap() else {
        panic!("batch")
    };
    batch
}

#[tokio::test]
async fn intake_commits_whole_projection_before_ack_and_never_exports_entity_authority() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("agent.sqlite");
    let journal = Journal::open(&db, JournalOptions::default()).await.unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let seen = acknowledgements.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let diff = reads.clone();
    let client = test_kit::client(
        session(&dir.path().join("session.sqlite")).await,
        ClientOptions::default(),
        move |_, bytes| {
            let db = db.clone();
            let seen = seen.clone();
            let diff = diff.clone();
            async move {
                if let Some(reply) = self_reply(&bytes, false) {
                    let count = seen.fetch_add(1, Ordering::SeqCst) + 1;
                    if count == 2 || count == 4 {
                        let c = rusqlite::Connection::open(&db).unwrap();
                        let rows: i64 = c
                            .query_row("SELECT COUNT(*) FROM mtproto_updates", [], |r| r.get(0))
                            .unwrap();
                        assert_eq!(
                            rows,
                            if count == 2 { 1 } else { 3 },
                            "journal commit must precede M2's ACK identity RPC"
                        );
                    }
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                    return Ok(state(1).to_bytes());
                }
                assert_eq!(
                    &bytes[..4],
                    &tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes()
                );
                if diff.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(difference(
                        vec![message(1, "hello"), message(2, "world")],
                        vec![],
                        3,
                    ))
                } else {
                    Ok(empty(3))
                }
            }
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    assert!(matches!(
        feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
            .unwrap(),
        Ingested::Batch(_)
    ));
    assert!(matches!(
        feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
            .unwrap(),
        Ingested::Batch(_)
    ));
    let page = feed.read(None, 100).await.unwrap();
    assert_eq!(page.events().len(), 3);
    assert!(page.events().iter().all(|e| e.checkpoint_acknowledged()));
    let Event::Message {
        peer, sender, text, ..
    } = page.events()[1].event()
    else {
        panic!("message")
    };
    assert_eq!(peer.bare_id(), 7);
    assert_eq!(sender.unwrap().bare_id(), 99);
    assert_eq!(text, "hello");
    assert_eq!(page.events()[1].event().peer().unwrap().bare_id(), 7);
    let wire = serde_json::to_string(page.events()).unwrap();
    assert!(!wire.contains("PRIVATE_"));
    assert!(!wire.contains("access_hash"));
    assert!(!wire.contains("777"));
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 4);
    assert!(matches!(
        feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
            .unwrap(),
        Ingested::Idle
    ));
    feed.release_source(&lease).await.unwrap();
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn encrypted_pending_replay_deduplicates_after_journal_commit_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("agent.sqlite");
    let source = dir.path().join("session.sqlite");
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let lease = feed.claim_source().await.unwrap();
    let client =
        test_kit::client(
            session(&source).await,
            ClientOptions::default(),
            |_, bytes| async move {
                Ok(self_reply(&bytes, false).unwrap_or_else(|| state(1).to_bytes()))
            },
        )
        .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    let pending = batch(&mut receiver).await;
    let id = pending.id();
    assert!(feed.store_batch(&lease, &pending).await.unwrap().inserted());
    assert!(!feed.store_batch(&lease, &pending).await.unwrap().inserted());
    assert!(!feed.read(None, 10).await.unwrap().events()[0].checkpoint_acknowledged());
    client.close().await.unwrap();
    drop(receiver);
    journal.close().await.unwrap();
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let client = test_kit::client(
        session(&source).await,
        ClientOptions::default(),
        |_, bytes| async move {
            Ok(self_reply(&bytes, false)
                .expect("pending replay must not read another state/difference"))
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::Resume)
        .unwrap();
    let replay = batch(&mut receiver).await;
    assert_eq!(replay.id(), id);
    assert!(!feed.store_batch(&lease, &replay).await.unwrap().inserted());
    drop(replay);
    let Ingested::Batch(receipt) = feed
        .ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap()
    else {
        panic!("pending")
    };
    assert!(!receipt.inserted());
    let page = feed.read(None, 10).await.unwrap();
    assert_eq!(page.events().len(), 1);
    assert!(page.events()[0].checkpoint_acknowledged());
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn gaps_and_explicit_resync_are_visible_without_a_guessed_ack() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let states = Arc::new(AtomicUsize::new(0));
    let seen = states.clone();
    let client = test_kit::client(
        session(&dir.path().join("session.sqlite")).await,
        ClientOptions::default(),
        move |_, bytes| {
            let states = seen.clone();
            async move {
                if let Some(reply) = self_reply(&bytes, false) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                    return Ok(state(if states.fetch_add(1, Ordering::SeqCst) == 0 {
                        1
                    } else {
                        50
                    })
                    .to_bytes());
                }
                Ok(
                    tl::enums::updates::Difference::TooLong(
                        tl::types::updates::DifferenceTooLong { pts: 50 },
                    )
                    .to_bytes(),
                )
            }
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    let Ingested::Gap(gap) = feed
        .ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap()
    else {
        panic!("gap")
    };
    assert!(matches!(
        feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
            .unwrap(),
        Ingested::Gap(_)
    ));
    assert_eq!(feed.read(None, 10).await.unwrap().events().len(), 2);
    receiver.resync_from_current(&gap).await.unwrap();
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    let page = feed.read(None, 10).await.unwrap();
    assert!(matches!(page.events()[1].event(), Event::Gap { .. }));
    assert!(!page.events()[1].checkpoint_acknowledged());
    assert!(matches!(page.events()[2].event(), Event::ResyncPrepared));
    assert!(page.events()[2].checkpoint_acknowledged());
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn shared_bot_mtproto_limits_roll_back_whole_batches_without_ack() {
    use tg_agent::updates::{SourceMode, UpdateFeed, UpdateOptions};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("global.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_update_entries(3))
        .await
        .unwrap();
    let bot = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let bs = bot.claim_source(SourceMode::Webhook).await.unwrap();
    bot.append_webhook(
        &bs,
        serde_json::from_value(serde_json::json!({"update_id":1})).unwrap(),
    )
    .await
    .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let client = test_kit::client(
        session(&dir.path().join("source.sqlite")).await,
        ClientOptions::default(),
        |_, bytes| async move {
            if let Some(reply) = self_reply(&bytes, false) {
                Ok(reply)
            } else if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                Ok(state(1).to_bytes())
            } else {
                Ok(difference(
                    vec![message(1, "one"), message(2, "two")],
                    vec![],
                    3,
                ))
            }
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    let error = feed
        .ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap_err();
    assert_eq!(
        error.journal_error().unwrap().kind(),
        JournalErrorKind::Capacity
    );
    assert_eq!(feed.read(None, 10).await.unwrap().events().len(), 1);
    let pending = batch(&mut receiver).await;
    assert_eq!(pending.updates().len(), 2);
    let id = pending.id();
    drop(pending);
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default().max_update_entries(4))
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    assert_eq!(batch(&mut receiver).await.id(), id);
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(feed.read(None, 10).await.unwrap().events().len(), 3);
    let bot = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
        .await
        .unwrap();
    let bs = bot.claim_source(SourceMode::Webhook).await.unwrap();
    assert_eq!(
        bot.append_webhook(
            &bs,
            serde_json::from_value(serde_json::json!({"update_id":2})).unwrap()
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Capacity
    );
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn private_callback_data_has_original_provenance_but_no_serialized_query_authority() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("callback.sqlite");
    let source = dir.path().join("source.sqlite");
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(true),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let client = test_kit::client(
        session(&source).await,
        ClientOptions::default(),
        |_, bytes| async move {
            if let Some(reply) = self_reply(&bytes, true) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(state(1).to_bytes());
            }
            Ok(difference(
                vec![],
                vec![tl::enums::Update::BotCallbackQuery(
                    tl::types::UpdateBotCallbackQuery {
                        query_id: 0,
                        user_id: 99,
                        peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 7 }),
                        msg_id: 1,
                        chat_instance: 888,
                        data: Some(b"data".to_vec()),
                        game_short_name: None,
                    },
                )],
                1,
            ))
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(true))
        .updates(UpdateMode::StartNow)
        .unwrap();
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    let pending = batch(&mut receiver).await;
    feed.store_batch(&lease, &pending).await.unwrap();
    let page = feed.read(None, 10).await.unwrap();
    let record = &page.events()[1];
    assert!(record.has_callback_query());
    assert!(record.is_from(&journal));
    assert!(record.callback_observed_at().is_some());
    let observed = record.callback_observed_at();
    assert_eq!(record.event().peer().unwrap().bare_id(), 7);
    let encoded = serde_json::to_string(record).unwrap();
    for key in [
        "query_id",
        "callback_query",
        "origin",
        "callback_received",
        "chat_instance",
        "access_hash",
    ] {
        assert!(!encoded.contains(key));
    }
    assert!(!format!("{record:?}").contains("data"));
    assert!(!feed.store_batch(&lease, &pending).await.unwrap().inserted());
    assert_eq!(
        feed.read(None, 10).await.unwrap().events()[1].callback_observed_at(),
        observed
    );
    client.close().await.unwrap();
    drop(receiver);
    journal.close().await.unwrap();
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(true),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let client = test_kit::client(
        session(&source).await,
        ClientOptions::default(),
        |_, bytes| async move { Ok(self_reply(&bytes, true).expect("replay")) },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(true))
        .updates(UpdateMode::Resume)
        .unwrap();
    assert!(
        !feed.read(None, 10).await.unwrap().events()[1].is_from(
            &Journal::open(dir.path().join("other.sqlite"), JournalOptions::default())
                .await
                .unwrap()
        )
    );
    let Ingested::Batch(receipt) = feed
        .ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap()
    else {
        panic!("replay")
    };
    assert!(!receipt.inserted());
    assert!(
        feed.read(None, 10).await.unwrap().events()[1]
            .callback_observed_at()
            .is_none()
    );
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn foreign_receiver_and_new_storage_refuse_before_rpc_until_explicit_source_change() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let first =
        test_kit::client(
            session(&dir.path().join("one.sqlite")).await,
            ClientOptions::default(),
            |_, bytes| async move {
                Ok(self_reply(&bytes, false).unwrap_or_else(|| state(1).to_bytes()))
            },
        )
        .unwrap();
    let mut receiver = first
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    first.close().await.unwrap();
    drop(receiver);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let second = test_kit::client(
        session(&dir.path().join("two.sqlite")).await,
        ClientOptions::default(),
        move |_, bytes| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(self_reply(&bytes, false).unwrap_or_else(|| state(1).to_bytes()))
            }
        },
    )
    .unwrap();
    let mut receiver = second
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    assert_eq!(
        feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
            .unwrap_err()
            .journal_error()
            .unwrap()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let old = lease.clone();
    lease = feed.accept_source_change(&lease, &receiver).await.unwrap();
    assert_eq!(
        feed.renew_source(&old).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap();
    let page = feed.read(None, 10).await.unwrap();
    assert_eq!(page.events().len(), 3);
    assert!(matches!(page.events()[1].event(), Event::SourceChanged));
    drop(receiver);
    let mut foreign = second
        .account(AccountIdentity {
            id: AccountId::new(43).unwrap(),
            kind: AccountKind::User,
        })
        .updates(UpdateMode::Resume)
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    assert_eq!(
        feed.ingest(&mut lease, &mut foreign, Duration::ZERO)
            .await
            .unwrap_err()
            .journal_error()
            .unwrap()
            .kind(),
        JournalErrorKind::Validation
    );
    assert_eq!(calls.load(Ordering::SeqCst), before);
    second.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn expired_rows_advance_floor_but_replay_never_reinserts_expired_payload() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let opts = Options::default().retention(Duration::from_millis(60));
    let feed = Feed::open(journal.clone(), "personal", account(false), opts)
        .await
        .unwrap();
    let lease = feed.claim_source().await.unwrap();
    let client =
        test_kit::client(
            session(&dir.path().join("source.sqlite")).await,
            ClientOptions::default(),
            |_, bytes| async move {
                Ok(self_reply(&bytes, false).unwrap_or_else(|| state(1).to_bytes()))
            },
        )
        .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    let pending = batch(&mut receiver).await;
    feed.store_batch(&lease, &pending).await.unwrap();
    let cursor = |profile: &str, sequence: i64| {
        format!(
            "tgmt1.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&serde_json::json!({"profile":profile,"sequence":sequence}))
                    .unwrap()
            )
        )
        .parse::<Cursor>()
        .unwrap()
    };
    assert_eq!(
        feed.read(Some(&cursor("other", 0)), 10)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert_eq!(
        feed.read(Some(&cursor("personal", 99)), 10)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    tokio::time::sleep(Duration::from_millis(90)).await;
    assert_eq!(
        feed.read(Some(&cursor("personal", 0)), 10)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::CursorExpired
    );
    assert_eq!(journal.prune_updates(100).await.unwrap(), 1);
    assert!(feed.read(None, 10).await.unwrap().events().is_empty());
    assert!(!feed.store_batch(&lease, &pending).await.unwrap().inserted());
    assert!(feed.read(None, 10).await.unwrap().events().is_empty());
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn caller_drop_during_ack_keeps_committed_data_and_replays_after_joined_close() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("cancel.sqlite");
    let source = dir.path().join("source.sqlite");
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = tokio::sync::oneshot::channel();
    let signal = Arc::new(std::sync::Mutex::new(Some(started)));
    let client = test_kit::client(
        session(&source).await,
        ClientOptions::default(),
        move |_, bytes| {
            let count = count.clone();
            let signal = signal.clone();
            async move {
                if let Some(reply) = self_reply(&bytes, false) {
                    if count.fetch_add(1, Ordering::SeqCst) == 1 {
                        signal.lock().unwrap().take().unwrap().send(()).unwrap();
                        std::future::pending::<()>().await;
                    }
                    return Ok(reply);
                }
                Ok(state(1).to_bytes())
            }
        },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::StartNow)
        .unwrap();
    let producer = feed.clone();
    let task = tokio::spawn(async move {
        producer
            .ingest(&mut lease, &mut receiver, Duration::ZERO)
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    let page = feed.read(None, 10).await.unwrap();
    assert_eq!(page.events().len(), 1);
    assert!(!page.events()[0].checkpoint_acknowledged());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), client.close())
        .await
        .unwrap()
        .unwrap();
    journal.close().await.unwrap();
    let journal = Journal::open(&agent, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        account(false),
        Options::default(),
    )
    .await
    .unwrap();
    let mut lease = feed.claim_source().await.unwrap();
    let client = test_kit::client(
        session(&source).await,
        ClientOptions::default(),
        |_, bytes| async move { Ok(self_reply(&bytes, false).expect("pending")) },
    )
    .unwrap();
    let mut receiver = client
        .account(expected(false))
        .updates(UpdateMode::Resume)
        .unwrap();
    let Ingested::Batch(receipt) = feed
        .ingest(&mut lease, &mut receiver, Duration::ZERO)
        .await
        .unwrap()
    else {
        panic!("replay")
    };
    assert!(!receipt.inserted());
    assert!(feed.read(None, 10).await.unwrap().events()[0].checkpoint_acknowledged());
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[test]
#[ignore = "synthetic subprocess fixture; invoked only by the crash boundary test"]
fn mtproto_intake_crash_child() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("TG_MTP_INTAKE_DIR").expect("test path"));
    let stage = std::env::var("TG_MTP_INTAKE_STAGE").unwrap();
    fn ready() {
        use std::io::Write;
        println!("MT_INTAKE_READY");
        std::io::stdout().flush().unwrap();
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let journal = Journal::open(
                directory.join("agent.sqlite"),
                JournalOptions::default().lease_duration(Duration::from_secs(2)),
            )
            .await
            .unwrap();
            let feed = Feed::open(journal, "personal", account(false), Options::default())
                .await
                .unwrap();
            let mut lease = feed.claim_source().await.unwrap();
            let session = Session::open(
                directory.join("source.sqlite"),
                SessionBinding::new("synthetic", 1).unwrap(),
                SessionKey::from_bytes([7; 32]),
                SessionOptions::default().lease(Duration::from_millis(800)),
            )
            .await
            .unwrap();
            let ack_calls = Arc::new(AtomicUsize::new(0));
            let selected = stage.clone();
            let client = test_kit::client(
                session,
                ClientOptions::default().request_timeout(Duration::from_millis(700)),
                move |_, bytes| {
                    let selected = selected.clone();
                    let ack_calls = ack_calls.clone();
                    async move {
                        if let Some(reply) = self_reply(&bytes, false) {
                            if ack_calls.fetch_add(1, Ordering::SeqCst) == 1
                                && selected == "after_append"
                            {
                                ready();
                                std::future::pending::<()>().await;
                            }
                            return Ok(reply);
                        }
                        if selected == "before_append" {
                            ready();
                            std::future::pending::<()>().await;
                        }
                        Ok(state(1).to_bytes())
                    }
                },
            )
            .unwrap();
            let mut receiver = client
                .account(expected(false))
                .updates(UpdateMode::StartNow)
                .unwrap();
            feed.ingest(&mut lease, &mut receiver, Duration::ZERO)
                .await
                .unwrap();
            assert_eq!(stage, "after_ack");
            ready();
            std::future::pending::<()>().await;
        });
}

#[test]
fn killed_process_preserves_commit_before_ack_at_all_three_boundaries() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for stage in ["before_append", "after_append", "after_ack"] {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "mtproto_intake_crash_child",
                    "--nocapture",
                ])
                .env("TG_MTP_INTAKE_DIR", dir.path())
                .env("TG_MTP_INTAKE_STAGE", stage)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let output = child.0.stdout.take().unwrap();
        let (signal, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if line.unwrap().trim() == "MT_INTAKE_READY" {
                    let _ = signal.send(());
                    break;
                }
            }
        });
        waiting
            .recv_timeout(Duration::from_secs(10))
            .expect("child boundary");
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        reader.join().unwrap();
        std::thread::sleep(Duration::from_millis(2200));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let journal = Journal::open(
                    dir.path().join("agent.sqlite"),
                    JournalOptions::default().lease_duration(Duration::from_secs(2)),
                )
                .await
                .unwrap();
                let feed = Feed::open(
                    journal.clone(),
                    "personal",
                    account(false),
                    Options::default(),
                )
                .await
                .unwrap();
                let mut lease = feed.claim_source().await.unwrap();
                let prior = feed.read(None, 10).await.unwrap();
                assert_eq!(prior.events().len(), usize::from(stage != "before_append"));
                if let Some(event) = prior.events().first() {
                    assert_eq!(event.checkpoint_acknowledged(), stage == "after_ack");
                }
                let client = test_kit::client(
                    session(&dir.path().join("source.sqlite")).await,
                    ClientOptions::default().request_timeout(Duration::from_millis(700)),
                    |_, bytes| async move {
                        if let Some(reply) = self_reply(&bytes, false) {
                            Ok(reply)
                        } else if bytes[..4]
                            == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes()
                        {
                            Ok(state(1).to_bytes())
                        } else {
                            Ok(empty(1))
                        }
                    },
                )
                .unwrap();
                let mode = if stage == "before_append" {
                    UpdateMode::StartNow
                } else {
                    UpdateMode::Resume
                };
                let mut receiver = client.account(expected(false)).updates(mode).unwrap();
                match feed
                    .ingest(&mut lease, &mut receiver, Duration::ZERO)
                    .await
                    .unwrap()
                {
                    Ingested::Batch(r) => assert_eq!(r.inserted(), stage == "before_append"),
                    Ingested::Idle => assert_eq!(stage, "after_ack"),
                    _ => panic!("unexpected gap"),
                }
                let page = feed.read(None, 10).await.unwrap();
                assert_eq!(page.events().len(), 1);
                assert!(page.events()[0].checkpoint_acknowledged());
                client.close().await.unwrap();
                journal.close().await.unwrap();
            });
    }
}
