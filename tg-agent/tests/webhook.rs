#![cfg(feature = "webhook")]
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};
use tg::{
    Bot, CancellationToken,
    test_kit::{MockResponse, MockTransport, Step},
    webhook::{WebhookOptions, WebhookSecret},
};
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    runtime::RuntimeError,
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};
const SECRET: &str = "SYNTHETIC_DURABLE_SECRET";
fn event(id: i64, text: &str) -> Value {
    json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"},"text":text},"future_metadata":{"private":"preserved"}})
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
async fn post(url: &str, event: Value) -> reqwest::Result<reqwest::Response> {
    client()
        .post(url)
        .header("x-telegram-bot-api-secret-token", SECRET)
        .json(&event)
        .send()
        .await
}
struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    journal: Journal,
    feed: UpdateFeed,
    bot: Bot,
    mock: MockTransport,
    id: i64,
}
impl Fixture {
    async fn new(options: JournalOptions, identities: usize) -> Self {
        static NEXT: AtomicI64 = AtomicI64::new(8_200_001);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mock = MockTransport::new((0..identities).map(|_| {
            Step::reply(
                "getMe",
                MockResponse::json(
                    json!({"ok":true,"result":{"id":id,"is_bot":true,"first_name":"Offline"}}),
                ),
            )
        }))
        .unwrap();
        let bot = Bot::builder(format!("{id}:OFFLINE_WEBHOOK"))
            .mock_transport(mock.clone())
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.sqlite");
        let journal = Journal::open(&path, options).await.unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", id, UpdateOptions::default())
            .await
            .unwrap();
        Self {
            _directory: directory,
            path,
            journal,
            feed,
            bot,
            mock,
            id,
        }
    }
    async fn start(&self, options: WebhookOptions) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/telegram", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let task = {
            let feed = self.feed.clone();
            let bot = self.bot.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                feed.webhook(&bot, WebhookSecret::new(SECRET).unwrap())
                    .options(options)
                    .serve_until(listener, stop)
                    .await
            })
        };
        let server = Server {
            url,
            stop,
            task: Some(task),
        };
        assert_eq!(
            client().get(&server.url).send().await.unwrap().status(),
            405
        );
        server
    }
}
struct Server {
    url: String,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), RuntimeError>>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Server {
    async fn finish(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn committed_duplicates_conflicts_quota_and_restart_preserve_the_feed() {
    let mut fixture = Fixture::new(JournalOptions::default().max_update_entries(1), 2).await;
    let server = fixture.start(WebhookOptions::default()).await;
    assert_eq!(
        post(&server.url, event(1, "one")).await.unwrap().status(),
        200
    );
    assert_eq!(
        post(&server.url, event(1, "one")).await.unwrap().status(),
        200
    );
    assert_eq!(
        post(&server.url, event(1, "changed"))
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        post(&server.url, event(2, "two")).await.unwrap().status(),
        503
    );
    assert_eq!(
        fixture.bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    assert_eq!(
        fixture
            .feed
            .claim_source(SourceMode::Polling)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    let page = fixture.feed.read(None, 100).await.unwrap();
    assert_eq!(page.events().len(), 1);
    assert_eq!(
        page.events()[0].update().unknown_fields()["future_metadata"]["private"],
        "preserved"
    );
    server.finish().await;
    fixture.journal.close().await.unwrap();
    fixture.journal = Journal::open(
        &fixture.path,
        JournalOptions::default().max_update_entries(1),
    )
    .await
    .unwrap();
    fixture.feed = UpdateFeed::open(
        fixture.journal.clone(),
        "bot",
        fixture.id,
        UpdateOptions::default(),
    )
    .await
    .unwrap();
    let server = fixture.start(WebhookOptions::default()).await;
    assert_eq!(
        post(&server.url, event(1, "one")).await.unwrap().status(),
        200
    );
    assert_eq!(
        fixture.feed.read(None, 100).await.unwrap().events().len(),
        1
    );
    server.finish().await;
    fixture.mock.assert_finished().unwrap();
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn http_success_waits_for_sqlite_commit() {
    let fixture = Fixture::new(JournalOptions::default(), 1).await;
    let server = fixture.start(WebhookOptions::default()).await;
    let mut blocker = rusqlite::Connection::open(&fixture.path).unwrap();
    let lock = blocker
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let url = server.url.clone();
    let request = post(&url, event(1, "one"));
    tokio::pin!(request);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut request)
            .await
            .is_err(),
        "no 2xx before commit"
    );
    let stored: i64 = lock
        .query_row("SELECT COUNT(*) FROM updates", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, 0);
    lock.commit().unwrap();
    assert_eq!(request.await.unwrap().status(), 200);
    assert_eq!(
        fixture.feed.read(None, 100).await.unwrap().events().len(),
        1
    );
    server.finish().await;
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn timed_out_native_append_is_fenced_before_source_release() {
    let fixture = Fixture::new(JournalOptions::default().max_pending(1), 1).await;
    let mut server = fixture
        .start(WebhookOptions::default().request_timeout(Duration::from_millis(25)))
        .await;
    let mut blocker = rusqlite::Connection::open(&fixture.path).unwrap();
    let lock = blocker
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        post(&server.url, event(1, "one")).await.unwrap().status(),
        504
    );
    server.stop.cancel();
    let task = server.task.as_mut().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), task)
            .await
            .is_err()
    );
    assert_eq!(
        fixture.bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    lock.commit().unwrap();
    tokio::time::timeout(Duration::from_secs(3), server.task.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.feed.read(None, 100).await.unwrap().events().len(),
        1
    );
    fixture.bot.claim_update_source().unwrap();
    let source = fixture
        .feed
        .claim_source(SourceMode::Polling)
        .await
        .unwrap();
    fixture.feed.release_source(source).await.unwrap();
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn idle_heartbeat_preserves_ownership_and_foreign_takeover_fences_the_old_source() {
    let config = JournalOptions::default().lease_duration(Duration::from_millis(600));
    let fixture = Fixture::new(config.clone(), 1).await;
    let mut server = fixture
        .start(WebhookOptions::default().request_timeout(Duration::from_millis(100)))
        .await;
    let rival = Journal::open(&fixture.path, config).await.unwrap();
    let feed = UpdateFeed::open(rival.clone(), "bot", fixture.id, UpdateOptions::default())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        feed.claim_source(SourceMode::Polling)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    rusqlite::Connection::open(&fixture.path)
        .unwrap()
        .execute("UPDATE update_feeds SET source_until_ms=0", [])
        .unwrap();
    let lease = feed.claim_source(SourceMode::Polling).await.unwrap();
    if let Ok(response) = post(&server.url, event(1, "stale")).await {
        assert_eq!(response.status(), 503);
    }
    let error = tokio::time::timeout(Duration::from_secs(3), server.task.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error,RuntimeError::Journal(error) if error.kind()==JournalErrorKind::LeaseLost)
    );
    feed.renew_source(&lease).await.unwrap();
    assert!(feed.read(None, 100).await.unwrap().events().is_empty());
    fixture.bot.claim_update_source().unwrap();
    fixture.journal.close().await.unwrap();
    rival.close().await.unwrap();
}

#[tokio::test]
async fn blocked_renewal_closes_intake_before_expiry_and_cleanup_waits_for_native_work() {
    let fixture = Fixture::new(
        JournalOptions::default()
            .max_pending(1)
            .lease_duration(Duration::from_millis(1200)),
        1,
    )
    .await;
    let mut server = fixture
        .start(WebhookOptions::default().request_timeout(Duration::from_millis(200)))
        .await;
    let mut blocker = rusqlite::Connection::open(&fixture.path).unwrap();
    let lock = blocker
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let until: i64 = lock
        .query_row("SELECT source_until_ms FROM update_feeds", [], |row| {
            row.get(0)
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(950)).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        now < until,
        "fixture must observe the closure before the known lease expires"
    );
    assert!(
        client().get(&server.url).send().await.is_err(),
        "no further sockets should be admitted after renewal loses its deadline"
    );
    assert!(!server.task.as_ref().unwrap().is_finished());
    assert_eq!(
        fixture.bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    lock.commit().unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), server.task.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error,RuntimeError::Journal(error) if error.kind()==JournalErrorKind::LeaseLost)
    );
    fixture.bot.claim_update_source().unwrap();
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn startup_is_lazy_and_rejects_invalid_budgets_or_feed_identity_before_claiming() {
    for mode in ["budget", "identity", "cancelled"] {
        let fixture =
            Fixture::new(JournalOptions::default(), usize::from(mode == "identity")).await;
        let other = if mode == "identity" {
            Some(
                Journal::open(
                    fixture._directory.path().join("other.sqlite"),
                    JournalOptions::default(),
                )
                .await
                .unwrap(),
            )
        } else {
            None
        };
        let feed = if let Some(journal) = &other {
            UpdateFeed::open(
                journal.clone(),
                "other",
                fixture.id + 1,
                UpdateOptions::default(),
            )
            .await
            .unwrap()
        } else {
            fixture.feed.clone()
        };
        let options = if mode == "budget" {
            WebhookOptions::default().request_timeout(Duration::from_secs(31))
        } else {
            WebhookOptions::default()
        };
        let builder = feed
            .webhook(&fixture.bot, WebhookSecret::new(SECRET).unwrap())
            .options(options);
        assert!(fixture.mock.requests().unwrap().is_empty());
        let stop = CancellationToken::new();
        if mode == "cancelled" {
            stop.cancel();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let result = builder.serve_until(listener, stop).await;
        if mode == "cancelled" {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(RuntimeError::Configuration)));
        }
        fixture.mock.assert_finished().unwrap();
        fixture.bot.claim_update_source().unwrap();
        let lease = feed.claim_source(SourceMode::Polling).await.unwrap();
        feed.release_source(lease).await.unwrap();
        if let Some(journal) = other {
            journal.close().await.unwrap();
        }
        fixture.journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn dropping_the_runtime_leaves_only_a_bounded_database_lease() {
    let fixture = Fixture::new(JournalOptions::default(), 1).await;
    let mut server = fixture.start(WebhookOptions::default()).await;
    let task = server.task.take().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.bot.claim_update_source().unwrap();
    assert_eq!(
        fixture
            .feed
            .claim_source(SourceMode::Polling)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    rusqlite::Connection::open(&fixture.path)
        .unwrap()
        .execute("UPDATE update_feeds SET source_until_ms=0", [])
        .unwrap();
    let lease = fixture
        .feed
        .claim_source(SourceMode::Polling)
        .await
        .unwrap();
    fixture.feed.release_source(lease).await.unwrap();
    fixture.journal.close().await.unwrap();
}
