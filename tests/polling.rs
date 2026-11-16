#![cfg(feature = "bot")]

use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg::{
    Bot, CancellationToken, EffectOutcome, Error, ErrorKind, MessageContext, PollingOptions, Router,
};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::path};

struct Fixture {
    server: MockServer,
    bot: Bot,
    token: String,
}

impl Fixture {
    async fn new() -> Self {
        static NEXT: AtomicI64 = AtomicI64::new(800000);
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let token = format!("{id}:OFFLINE_TEST_TOKEN");
        let server = MockServer::start().await;
        for (method, result) in [
            (
                "getMe",
                json!({"id":id,"is_bot":true,"first_name":"Demo","username":"demo_bot"}),
            ),
            (
                "getWebhookInfo",
                json!({"url":"","has_custom_certificate":false,"pending_update_count":0}),
            ),
        ] {
            Mock::given(path(format!("/bot{token}/{method}")))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":result})),
                )
                .mount(&server)
                .await;
        }
        let bot = Bot::builder(&token)
            .api_url(server.uri())
            .allow_http_loopback(true)
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        Self { server, bot, token }
    }
    fn updates_path(&self) -> String {
        format!("/bot{}/getUpdates", self.token)
    }
}

fn options() -> PollingOptions {
    PollingOptions::default()
        .poll_timeout(0)
        .max_concurrency(2)
        .handler_timeout(Duration::from_secs(3))
        .shutdown_timeout(Duration::from_secs(1))
}

fn event(id: i64, chat: i64, text: &str) -> Value {
    json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"},"text":text}})
}

#[derive(Default)]
struct State {
    active: AtomicUsize,
    peak: AtomicUsize,
    done: Mutex<Vec<String>>,
    first_started: Notify,
    other_done: Notify,
    release: Notify,
}

#[tokio::test]
async fn offsets_wait_for_the_batch_and_chats_execute_in_order_with_a_bound() {
    let fixture = Fixture::new().await;
    let stop = CancellationToken::new();
    let signal_stop = stop.clone();
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = calls.clone();
    Mock::given(path(fixture.updates_path()))
        .respond_with(move |request: &Request| {
            let mut calls = captured.lock().unwrap();
            calls.push(request.body_json().unwrap());
            if calls.len() == 1 {
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[
                    event(10,1,"first"),event(11,1,"second"),event(20,2,"other")
                ]}))
            } else {
                signal_stop.cancel();
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[]}))
            }
        })
        .expect(2)
        .mount(&fixture.server)
        .await;
    let state = Arc::new(State::default());
    let shared = state.clone();
    let router =
        Router::with_state(shared).on_message(|ctx: MessageContext<Arc<State>>| async move {
            let active = ctx.state().active.fetch_add(1, Ordering::SeqCst) + 1;
            ctx.state().peak.fetch_max(active, Ordering::SeqCst);
            let text = ctx.message.text.as_deref().unwrap();
            if text == "first" {
                ctx.state().first_started.notify_one();
                ctx.state().release.notified().await;
            }
            ctx.state().done.lock().unwrap().push(text.to_owned());
            ctx.state().active.fetch_sub(1, Ordering::SeqCst);
            if text == "other" {
                ctx.state().other_done.notify_one();
            }
            Ok(())
        });
    let bot = fixture.bot.clone();
    let task = tokio::spawn(async move { bot.run_polling_until(router, options(), stop).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        state.first_started.notified().await;
        state.other_done.notified().await;
    })
    .await
    .unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "must not acknowledge a still-running batch"
    );
    assert_eq!(*state.done.lock().unwrap(), vec!["other"]);
    state.release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(
        *state.done.lock().unwrap(),
        vec!["other", "first", "second"]
    );
    assert_eq!(state.peak.load(Ordering::SeqCst), 2);
    let calls = calls.lock().unwrap();
    assert!(calls[0].get("offset").is_none());
    assert_eq!(
        calls[1]["offset"], 21,
        "ID gaps do not require invented intermediate updates"
    );
}

#[tokio::test]
async fn handler_failure_stops_without_acknowledging_the_batch() {
    let fixture = Fixture::new().await;
    Mock::given(path(fixture.updates_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"ok":true,"result":[event(1,1,"first"),event(2,1,"second")]}),
            ),
        )
        .expect(1)
        .mount(&fixture.server)
        .await;
    let error = fixture
        .bot
        .run_polling_until(
            Router::new()
                .on_message(|_: MessageContext| async { Err(Error::handler("application")) }),
            options(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Handler);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}

#[tokio::test]
async fn a_second_instance_cannot_poll_the_same_bot_and_ownership_is_released() {
    let fixture = Fixture::new().await;
    let entered = Arc::new(Notify::new());
    let signal = entered.clone();
    Mock::given(path(fixture.updates_path()))
        .respond_with(move |_: &Request| {
            signal.notify_one();
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(json!({"ok":true,"result":[]}))
        })
        .expect(2)
        .mount(&fixture.server)
        .await;
    let stop = CancellationToken::new();
    let first_stop = stop.clone();
    let first_bot = fixture.bot.clone();
    let first = tokio::spawn(async move {
        first_bot
            .run_polling_until(Router::new(), options(), first_stop)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let second = Bot::builder(&fixture.token)
        .api_url(fixture.server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap();
    let error = second
        .run_polling_until(Router::new(), options(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Capacity);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    stop.cancel();
    first.await.unwrap().unwrap();

    let restart_stop = CancellationToken::new();
    let signal = restart_stop.clone();
    let restarted = tokio::spawn(async move {
        second
            .run_polling_until(Router::new(), options(), signal)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    restart_stop.cancel();
    restarted.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_active_webhook_is_not_deleted_or_bypassed() {
    let fixture = Fixture::new().await;
    Mock::given(path(format!("/bot{}/getWebhookInfo", fixture.token)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
            "url":"https://example.com/hook","has_custom_certificate":false,"pending_update_count":0
        }}))).with_priority(1).mount(&fixture.server).await;
    let error = fixture
        .bot
        .run_polling_until(Router::new(), options(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    assert_eq!(error.field(), Some("webhook_active"));
    let requests = fixture.server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path().ends_with("/getUpdates")
                || r.url.path().ends_with("/deleteWebhook"))
    );
}

#[tokio::test]
async fn shutdown_drains_the_accepted_batch_without_fetching_more() {
    let fixture = Fixture::new().await;
    Mock::given(path(fixture.updates_path()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok":true,"result":[event(1,1,"one")]})),
        )
        .expect(1)
        .mount(&fixture.server)
        .await;
    let state = Arc::new(State::default());
    let router = Router::with_state(state.clone()).on_message(
        |ctx: MessageContext<Arc<State>>| async move {
            ctx.state().first_started.notify_one();
            ctx.state().release.notified().await;
            ctx.state().done.lock().unwrap().push("done".into());
            Ok(())
        },
    );
    let stop = CancellationToken::new();
    let signal = stop.clone();
    let task = tokio::spawn(async move {
        fixture
            .bot
            .run_polling_until(router, options(), signal)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), state.first_started.notified())
        .await
        .unwrap();
    stop.cancel();
    assert!(!task.is_finished());
    state.release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(*state.done.lock().unwrap(), vec!["done"]);
}

#[tokio::test]
async fn cancelled_or_invalid_startup_does_not_touch_the_network() {
    let fixture = Fixture::new().await;
    let stop = CancellationToken::new();
    stop.cancel();
    fixture
        .bot
        .run_polling_until(Router::new(), options(), stop)
        .await
        .unwrap();
    let error = fixture
        .bot
        .run_polling_until(
            Router::new(),
            options().batch_limit(0),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Validation);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_unscoped_future_event_is_a_barrier_for_known_chat_events() {
    let fixture = Fixture::new().await;
    let stop = CancellationToken::new();
    let signal = stop.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    Mock::given(path(fixture.updates_path()))
        .respond_with(move |_: &Request| {
            if captured.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[
                    {"update_id":1,"future_event":{"unknown_scope":true}},event(2,1,"known")
                ]}))
            } else {
                signal.cancel();
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[]}))
            }
        })
        .expect(2)
        .mount(&fixture.server)
        .await;
    let state = Arc::new(State::default());
    let router = Router::with_state(state.clone()).on_update(
        |ctx: tg::UpdateContext<Arc<State>>| async move {
            if ctx.update.update_id == 1 {
                ctx.state().first_started.notify_one();
                ctx.state().release.notified().await;
            } else {
                ctx.state().other_done.notify_one();
            }
            Ok(())
        },
    );
    let task =
        tokio::spawn(async move { fixture.bot.run_polling_until(router, options(), stop).await });
    tokio::time::timeout(Duration::from_secs(2), state.first_started.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), state.other_done.notified())
            .await
            .is_err(),
        "an event with unknown scope must not run concurrently with a potentially related chat"
    );
    state.release.notify_one();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn panic_is_reported_without_ack_and_does_not_leak_the_owner() {
    async fn panicking(_: MessageContext) -> tg::Result<()> {
        panic!("synthetic private panic payload");
    }
    let fixture = Fixture::new().await;
    Mock::given(path(fixture.updates_path()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok":true,"result":[event(1,1,"one")]})),
        )
        .expect(2)
        .mount(&fixture.server)
        .await;
    for _ in 0..2 {
        let error = fixture
            .bot
            .run_polling_until(
                Router::new().on_message(panicking),
                options(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Handler);
        assert_eq!(error.outcome(), EffectOutcome::Unknown);
        assert!(!format!("{error:?} {error}").contains("synthetic private"));
    }
}

#[tokio::test]
async fn handler_and_shutdown_deadlines_stop_without_ack() {
    for cancel in [false, true] {
        let fixture = Fixture::new().await;
        Mock::given(path(fixture.updates_path()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok":true,"result":[event(1,1,"one")]})),
            )
            .expect(1)
            .mount(&fixture.server)
            .await;
        let entered = Arc::new(Notify::new());
        let router = Router::with_state(entered.clone()).on_message(
            |ctx: MessageContext<Arc<Notify>>| async move {
                ctx.state().notify_one();
                std::future::pending::<tg::Result<()>>().await
            },
        );
        let stop = CancellationToken::new();
        let signal = stop.clone();
        let config = if cancel {
            options().shutdown_timeout(Duration::from_millis(20))
        } else {
            options().handler_timeout(Duration::from_millis(20))
        };
        let task =
            tokio::spawn(
                async move { fixture.bot.run_polling_until(router, config, signal).await },
            );
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if cancel {
            stop.cancel();
        }
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Timeout);
        assert_eq!(error.outcome(), EffectOutcome::Unknown);
    }
}

#[tokio::test]
async fn malformed_batch_is_rejected_before_any_handler_runs() {
    for (batch, limit) in [
        (vec![event(2, 1, "two"), event(1, 1, "one")], 100),
        (vec![event(1, 1, "one"), event(1, 1, "duplicate")], 100),
        (vec![event(i64::MAX, 1, "overflow")], 100),
        (vec![event(1, 1, "one"), event(2, 1, "two")], 1),
    ] {
        let fixture = Fixture::new().await;
        Mock::given(path(fixture.updates_path()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":batch})),
            )
            .expect(1)
            .mount(&fixture.server)
            .await;
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::with_state(calls.clone()).on_message(
            |ctx: MessageContext<Arc<AtomicUsize>>| async move {
                ctx.state().fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let error = fixture
            .bot
            .run_polling_until(
                router,
                options().batch_limit(limit),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Decode);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn wrong_bot_identity_or_username_cannot_start_intake() {
    for wrong_id in [false, true] {
        let fixture = Fixture::new().await;
        if wrong_id {
            Mock::given(path(format!("/bot{}/getMe", fixture.token)))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
                        "id":1,"is_bot":true,"first_name":"Other","username":"demo_bot"
                    }})),
                )
                .with_priority(1)
                .mount(&fixture.server)
                .await;
        }
        let router = if wrong_id {
            Router::new()
        } else {
            Router::new().bot_username("other_bot")
        };
        let error = fixture
            .bot
            .run_polling_until(router, options(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
        assert!(
            !fixture
                .server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| request.url.path().ends_with("/getUpdates"))
        );
    }
}
