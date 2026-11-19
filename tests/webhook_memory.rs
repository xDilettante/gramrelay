#![cfg(feature = "webhook")]
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg::{
    Bot, CancellationToken, Router,
    webhook::{MemoryWebhookOptions, WebhookOptions, WebhookSecret},
};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
const SECRET: &str = "SYNTHETIC_MEMORY_SECRET";
fn event(id: i64, chat: i64, text: &str) -> Value {
    json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"},"text":text}})
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
async fn post(client: &reqwest::Client, url: &str, event: Value) -> reqwest::Response {
    client
        .post(url)
        .header("x-telegram-bot-api-secret-token", SECRET)
        .json(&event)
        .send()
        .await
        .unwrap()
}
struct Harness {
    url: String,
    bot: Bot,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<tg::Result<()>>>,
    _identity: MockServer,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Harness {
    async fn start<S: Send + Sync + 'static>(
        router: Router<S>,
        memory: MemoryWebhookOptions,
        options: WebhookOptions,
    ) -> Self {
        static NEXT: AtomicI64 = AtomicI64::new(9_900_001);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let identity = MockServer::start().await;
        let token = format!("{id}:OFFLINE_MEMORY");
        Mock::given(path(format!("/bot{token}/getMe"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"id":id,"is_bot":true,"first_name":"Fixture","username":"fixture_bot"}}))).expect(1).mount(&identity).await;
        let bot = Bot::builder(token)
            .api_url(identity.uri())
            .allow_http_loopback(true)
            .build()
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/telegram", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let task = {
            let bot = bot.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                bot.webhook(router, WebhookSecret::new(SECRET).unwrap())
                    .options(options)
                    .memory_options(memory)
                    .serve_until(listener, stop)
                    .await
            })
        };
        Self {
            url,
            bot,
            stop,
            task: Some(task),
            _identity: identity,
        }
    }
    async fn finish(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.bot.claim_update_source().unwrap();
    }
}

#[tokio::test]
async fn duplicate_processing_waits_and_matching_successes_do_not_run_twice() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let state = (started.clone(), release.clone(), calls.clone());
    let router = Router::with_state(state).on_message(|ctx| async move {
        ctx.state().2.fetch_add(1, Ordering::SeqCst);
        ctx.state().0.notify_one();
        ctx.state().1.notified().await;
        Ok(())
    });
    let server = Harness::start(
        router,
        MemoryWebhookOptions::default(),
        WebhookOptions::default(),
    )
    .await;
    let first = {
        let url = server.url.clone();
        tokio::spawn(async move { post(&client(), &url, event(1, 1, "one")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert!(!first.is_finished());
    assert_eq!(
        server.bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    assert_eq!(
        post(&client(), &server.url, event(1, 1, "one"))
            .await
            .status(),
        503
    );
    assert_eq!(
        post(&client(), &server.url, event(1, 1, "changed"))
            .await
            .status(),
        409
    );
    release.notify_one();
    assert_eq!(first.await.unwrap().status(), 200);
    assert_eq!(
        post(&client(), &server.url, event(1, 1, "one"))
            .await
            .status(),
        200
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.finish().await;
}

#[tokio::test]
async fn a_busy_chat_does_not_block_an_unrelated_chat_and_keeps_its_own_order() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let finished = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state((started.clone(), release.clone(), finished.clone()))
        .on_update(|ctx| async move {
            match ctx.update.update_id {
                1 => {
                    ctx.state().0.notify_one();
                    ctx.state().1.notified().await;
                    ctx.state().2.store(1, Ordering::SeqCst);
                }
                2 => assert_eq!(
                    ctx.state().2.load(Ordering::SeqCst),
                    1,
                    "same-chat admission must remain ordered"
                ),
                3 => assert_eq!(
                    ctx.state().2.load(Ordering::SeqCst),
                    0,
                    "an unrelated chat must run while the first is still blocked"
                ),
                _ => unreachable!(),
            }
            Ok(())
        });
    let server = Harness::start(
        router,
        MemoryWebhookOptions::default(),
        WebhookOptions::default(),
    )
    .await;
    let first = {
        let url = server.url.clone();
        tokio::spawn(async move { post(&client(), &url, event(1, 1, "one")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let second = {
        let url = server.url.clone();
        tokio::spawn(async move { post(&client(), &url, event(2, 1, "two")).await })
    };
    assert_eq!(
        post(&client(), &server.url, event(3, 2, "other"))
            .await
            .status(),
        200
    );
    assert!(!second.is_finished());
    release.notify_one();
    assert_eq!(first.await.unwrap().status(), 200);
    assert_eq!(second.await.unwrap().status(), 200);
    server.finish().await;
}

#[tokio::test]
async fn successful_cache_is_bounded_without_eviction_and_expiry_reclaims_capacity() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state(calls.clone()).on_update(|ctx| async move {
        ctx.state().fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let server = Harness::start(
        router,
        MemoryWebhookOptions::default()
            .dedup_capacity(1)
            .dedup_retention(Duration::from_millis(150)),
        WebhookOptions::default(),
    )
    .await;
    assert_eq!(
        post(&client(), &server.url, event(1, 1, "one"))
            .await
            .status(),
        200
    );
    assert_eq!(
        post(&client(), &server.url, event(2, 1, "two"))
            .await
            .status(),
        503
    );
    assert_eq!(
        post(&client(), &server.url, event(1, 1, "one"))
            .await
            .status(),
        200
    );
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert_eq!(
        post(&client(), &server.url, event(2, 1, "two"))
            .await
            .status(),
        200
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.finish().await;
}

#[tokio::test]
async fn memory_quota_survives_a_disconnected_http_waiter() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state((started.clone(), release.clone(), calls.clone())).on_update(
        |ctx| async move {
            ctx.state().2.fetch_add(1, Ordering::SeqCst);
            ctx.state().0.notify_one();
            ctx.state().1.notified().await;
            Ok(())
        },
    );
    let server = Harness::start(
        router,
        MemoryWebhookOptions::default().max_pending(1),
        WebhookOptions::default(),
    )
    .await;
    let first = {
        let url = server.url.clone();
        tokio::spawn(async move { post(&client(), &url, event(1, 1, "one")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    first.abort();
    let _ = first.await;
    assert_eq!(
        post(&client(), &server.url, event(2, 2, "other"))
            .await
            .status(),
        503
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = post(&client(), &server.url, event(1, 1, "one"))
                .await
                .status();
            if status == 200 {
                break;
            }
            assert_eq!(status, 503);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.finish().await;
}

#[tokio::test]
async fn failed_panicking_and_timed_out_handlers_are_not_cached_as_success() {
    for mode in ["error", "panic", "timeout"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::with_state(calls.clone()).on_update(move |ctx| async move {
            if ctx.state().fetch_add(1, Ordering::SeqCst) == 0 {
                match mode {
                    "panic" => panic!("synthetic handler panic"),
                    "timeout" => return std::future::pending().await,
                    _ => return Err(tg::Error::handler("synthetic")),
                }
            }
            Ok(())
        });
        let memory = MemoryWebhookOptions::default()
            .dispatch(tg::DispatchOptions::default().handler_timeout(Duration::from_millis(20)));
        let server = Harness::start(router, memory, WebhookOptions::default()).await;
        assert_eq!(
            post(&client(), &server.url, event(1, 1, "one"))
                .await
                .status(),
            if mode == "timeout" { 504 } else { 500 }
        );
        assert_eq!(
            post(&client(), &server.url, event(1, 1, "one"))
                .await
                .status(),
            200
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.finish().await;
    }
}

#[tokio::test]
async fn forced_stop_joins_active_handlers_before_releasing_source_ownership() {
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let active = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let router =
        Router::with_state((active.clone(), started.clone())).on_update(|ctx| async move {
            ctx.state().0.fetch_add(1, Ordering::SeqCst);
            let _active = Active(ctx.state().0.clone());
            ctx.state().1.notify_one();
            std::future::pending().await
        });
    let mut server = Harness::start(
        router,
        MemoryWebhookOptions::default(),
        WebhookOptions::default().shutdown_timeout(Duration::from_millis(20)),
    )
    .await;
    let request = {
        let url = server.url.clone();
        tokio::spawn(async move {
            client()
                .post(url)
                .header("x-telegram-bot-api-secret-token", SECRET)
                .json(&event(1, 1, "one"))
                .send()
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    server.stop.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), server.task.take().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), tg::ErrorKind::Timeout);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    server.bot.claim_update_source().unwrap();
    if let Ok(response) = request.await.unwrap() {
        assert!(!response.status().is_success());
    }
}

#[tokio::test]
async fn graceful_stop_confirms_accepted_work_and_holds_the_source_until_it_finishes() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let router =
        Router::with_state((started.clone(), release.clone())).on_update(|ctx| async move {
            ctx.state().0.notify_one();
            ctx.state().1.notified().await;
            Ok(())
        });
    let server = Harness::start(
        router,
        MemoryWebhookOptions::default(),
        WebhookOptions::default(),
    )
    .await;
    let first = {
        let url = server.url.clone();
        tokio::spawn(async move { post(&client(), &url, event(1, 1, "one")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    server.stop.cancel();
    assert_eq!(
        server.bot.claim_update_source().unwrap_err().kind(),
        tg::ErrorKind::Capacity
    );
    release.notify_one();
    assert_eq!(first.await.unwrap().status(), 200);
    server.finish().await;
}

#[tokio::test]
async fn lazy_startup_validates_configuration_identity_and_username_without_leaking_ownership() {
    for (index, mode) in ["config", "identity", "username", "cancelled"]
        .into_iter()
        .enumerate()
    {
        let id = 9_950_001 + index as i64;
        let token = format!("{id}:OFFLINE_STARTUP");
        let identity = MockServer::start().await;
        let expected = usize::from(!matches!(mode, "config" | "cancelled"));
        Mock::given(path(format!("/bot{token}/getMe"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"id":id+i64::from(mode=="identity"),"is_bot":true,"first_name":"Fixture","username":"fixture_bot"}}))).expect(expected as u64).mount(&identity).await;
        let bot = Bot::builder(token)
            .api_url(identity.uri())
            .allow_http_loopback(true)
            .build()
            .unwrap();
        let router = if mode == "username" {
            Router::new().bot_username("other_bot")
        } else {
            Router::new()
        };
        let memory = if mode == "config" {
            MemoryWebhookOptions::default().max_pending(0)
        } else {
            MemoryWebhookOptions::default()
        };
        let builder = bot
            .webhook(router, WebhookSecret::new(SECRET).unwrap())
            .memory_options(memory);
        assert!(
            identity.received_requests().await.unwrap().is_empty(),
            "constructing the builder is lazy"
        );
        let stop = CancellationToken::new();
        if mode == "cancelled" {
            stop.cancel();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let result = builder.serve_until(listener, stop).await;
        if mode == "cancelled" {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(
                error.kind(),
                if mode == "config" {
                    tg::ErrorKind::Validation
                } else {
                    tg::ErrorKind::Configuration
                }
            );
        }
        assert_eq!(identity.received_requests().await.unwrap().len(), expected);
        bot.claim_update_source().unwrap();
    }
}
