#![cfg(feature = "bot")]
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
use tg::{
    Bot, CancellationToken, Router,
    api::methods::GetUpdates,
    test_kit::{MockResponse, MockTransport, Step},
};
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    runtime::{ConsumerOptions, PollOptions, RuntimeError},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};
use tokio::sync::Notify;

fn event(id: i64) -> tg::Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"}}})).unwrap()
}
fn reply(method: &'static str, result: serde_json::Value) -> Step {
    Step::reply(
        method,
        MockResponse::json(json!({"ok":true,"result":result})),
    )
}
fn fixture(tail: Vec<Step>, webhook: &str) -> (i64, Bot, MockTransport) {
    static NEXT: AtomicI64 = AtomicI64::new(9_000_001);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut steps = vec![reply(
        "getMe",
        json!({"id":id,"is_bot":true,"first_name":"Fixture","username":"fixture_bot"}),
    )];
    if !webhook.eq("consumer") {
        steps.push(reply(
            "getWebhookInfo",
            json!({"url":webhook,"has_custom_certificate":false,"pending_update_count":0}),
        ));
    }
    steps.extend(tail);
    let mock = MockTransport::new(steps).unwrap();
    let bot = Bot::builder(format!("{id}:OFFLINE"))
        .mock_transport(mock.clone())
        .build()
        .unwrap();
    (id, bot, mock)
}
fn pending_poll() -> Step {
    Step::reply(
        "getUpdates",
        MockResponse::json(json!({"ok":true,"result":[]})).delay(Duration::from_secs(30)),
    )
}
async fn requests(mock: &MockTransport, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while mock.requests().unwrap().len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn polls() -> PollOptions {
    PollOptions::default().poll_timeout(0)
}

#[tokio::test]
async fn remote_ack_uses_only_committed_intake_and_stop_releases_both_owners() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let (id, bot, mock) = fixture(
        vec![reply("getUpdates", json!([event(10)])), pending_poll()],
        "",
    );
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let runner = {
        let feed = feed.clone();
        let bot = bot.clone();
        let stop = stop.clone();
        tokio::spawn(async move { feed.run_polling_until(&bot, polls(), stop).await })
    };
    requests(&mock, 4).await;
    assert_eq!(
        mock.requests().unwrap()[3]
            .decode::<GetUpdates>()
            .unwrap()
            .offset,
        Some(11)
    );
    assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 1);
    assert_eq!(
        bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    stop.cancel();
    runner.await.unwrap().unwrap();
    mock.assert_finished().unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(
        feed.poll_checkpoint(&source).await.unwrap().offset(),
        Some(11)
    );
    feed.release_source(source).await.unwrap();
    bot.claim_update_source().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn full_intake_rolls_back_without_acknowledging_a_received_batch() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default().max_update_entries(1),
    )
    .await
    .unwrap();
    let (id, bot, mock) = fixture(vec![reply("getUpdates", json!([event(10), event(11)]))], "");
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(feed.run_polling_until(&bot, polls(), CancellationToken::new()).await.unwrap_err(), RuntimeError::Journal(error) if error.kind()==JournalErrorKind::Capacity)
    );
    assert_eq!(mock.requests().unwrap().len(), 3);
    assert!(feed.read(None, 100).await.unwrap().events().is_empty());
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    assert_eq!(feed.poll_checkpoint(&source).await.unwrap().offset(), None);
    journal.close().await.unwrap();
}

#[tokio::test]
async fn restarted_collector_loads_committed_offset_before_its_first_request() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let (id, bot, mock) = fixture(vec![pending_poll()], "");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Polling).await.unwrap();
    let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
    feed.append_poll(&source, &checkpoint, vec![event(10)])
        .await
        .unwrap();
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let runner = {
        let feed = feed.clone();
        let stop = stop.clone();
        tokio::spawn(async move { feed.run_polling_until(&bot, polls(), stop).await })
    };
    requests(&mock, 3).await;
    assert_eq!(
        mock.requests().unwrap()[2]
            .decode::<GetUpdates>()
            .unwrap()
            .offset,
        Some(11)
    );
    stop.cancel();
    runner.await.unwrap().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn handler_error_panic_and_timeout_preserve_the_consumer_position() {
    for mode in ["error", "panic", "timeout"] {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("updates.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let (id, bot, _) = fixture(vec![], "consumer");
        let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
            .await
            .unwrap();
        let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
        feed.append_webhook(&source, event(1)).await.unwrap();
        let router = Router::new().on_update(move |_| async move {
            if mode == "panic" {
                panic!("synthetic handler panic");
            }
            if mode == "timeout" {
                return std::future::pending().await;
            }
            Err(tg::Error::handler("fixture_failure"))
        });
        let options = ConsumerOptions::default()
            .dispatch(tg::DispatchOptions::default().handler_timeout(Duration::from_millis(10)));
        assert!(matches!(
            feed.run_consumer_until(&bot, "router", router, options, CancellationToken::new())
                .await
                .unwrap_err(),
            RuntimeError::Telegram(_)
        ));
        let lease = feed.claim_consumer("router").await.unwrap();
        assert_eq!(
            feed.read_consumer(&lease, 100)
                .await
                .unwrap()
                .events()
                .len(),
            1
        );
        journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn active_webhooks_and_wrong_feed_identity_do_not_start_intake() {
    for wrong in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("updates.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let (id, bot, mock) = fixture(
            vec![],
            if wrong {
                ""
            } else {
                "https://example.invalid/hook"
            },
        );
        let feed = UpdateFeed::open(
            journal.clone(),
            "bot",
            id + i64::from(wrong),
            UpdateOptions::default(),
        )
        .await
        .unwrap();
        assert!(matches!(
            feed.run_polling_until(&bot, polls(), CancellationToken::new())
                .await
                .unwrap_err(),
            RuntimeError::Configuration
        ));
        assert!(
            mock.requests()
                .unwrap()
                .iter()
                .all(|r| r.method() != "getUpdates")
        );
        journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn slow_consumer_renews_its_lease_and_drains_before_persisting_ack() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let options = JournalOptions::default().lease_duration(Duration::from_millis(180));
    let journal = Journal::open(&path, options.clone()).await.unwrap();
    let rival = Journal::open(&path, options).await.unwrap();
    let (id, bot, mock) = fixture(vec![], "consumer");
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let other = UpdateFeed::open(rival.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source, event(1)).await.unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let state = (started.clone(), release.clone());
    let router = Router::with_state(state).on_update(|ctx| async move {
        ctx.state().0.notify_one();
        ctx.state().1.notified().await;
        Ok(())
    });
    let stop = CancellationToken::new();
    let runner = {
        let feed = feed.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            feed.run_consumer_until(
                &bot,
                "router",
                router,
                ConsumerOptions::default().idle_delay(Duration::from_millis(5)),
                stop,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(360)).await;
    assert_eq!(
        other.claim_consumer("router").await.unwrap_err().kind(),
        JournalErrorKind::Capacity
    );
    stop.cancel();
    assert!(!runner.is_finished());
    release.notify_one();
    runner.await.unwrap().unwrap();
    mock.assert_finished().unwrap();
    let lease = other.claim_consumer("router").await.unwrap();
    assert!(
        other
            .read_consumer(&lease, 100)
            .await
            .unwrap()
            .events()
            .is_empty()
    );
    journal.close().await.unwrap();
    rival.close().await.unwrap();
}

#[tokio::test]
async fn consumer_deadline_aborts_work_and_preserves_the_unacknowledged_position() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("updates.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let (id, bot, _) = fixture(vec![], "consumer");
    let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source, event(1)).await.unwrap();
    let started = Arc::new(Notify::new());
    let notified = started.clone();
    let router = Router::new().on_update(move |_| {
        let notified = notified.clone();
        async move {
            notified.notify_one();
            std::future::pending().await
        }
    });
    let stop = CancellationToken::new();
    let runner = {
        let feed = feed.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            feed.run_consumer_until(
                &bot,
                "router",
                router,
                ConsumerOptions::default().shutdown_timeout(Duration::from_millis(15)),
                stop,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    stop.cancel();
    assert!(matches!(
        runner.await.unwrap().unwrap_err(),
        RuntimeError::ShutdownTimeout
    ));
    let lease = feed.claim_consumer("router").await.unwrap();
    assert_eq!(
        feed.read_consumer(&lease, 100)
            .await
            .unwrap()
            .events()
            .len(),
        1
    );
    journal.close().await.unwrap();
}
