use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tg::{Bot, ChatId, EffectOutcome, ErrorKind, InputFile, RetryPolicy, UserId};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";
fn bot(server: &MockServer) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap()
}
fn policy() -> RetryPolicy {
    RetryPolicy::default()
        .jitter(false)
        .initial_delay(Duration::ZERO)
        .max_delay(Duration::from_millis(50))
        .max_elapsed(Duration::from_secs(1))
}
fn identity() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"ok":true,"result":{"id":123456,"is_bot":true,"first_name":"Bot"}}))
}
fn message() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}}),
    )
}
fn rate_limit(seconds: u64) -> ResponseTemplate {
    ResponseTemplate::new(429)
        .set_body_json(json!({"ok":false,"error_code":429,"parameters":{"retry_after":seconds}}))
}

#[tokio::test]
async fn retries_are_opt_in_and_reviewed_reads_recover_from_proxy_failures() {
    for enabled in [false, true] {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        Mock::given(method("POST"))
            .respond_with(move |_: &Request| {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(503).set_body_string("synthetic private proxy body")
                } else {
                    identity()
                }
            })
            .expect(if enabled { 2 } else { 1 })
            .mount(&server)
            .await;
        let bot = bot(&server);
        let mut request = bot.api().get_me();
        if enabled {
            request = request.retry_policy(policy());
        }
        if enabled {
            request.await.unwrap();
        } else {
            assert!(request.await.is_err());
        }
    }
}

#[tokio::test]
async fn confirmed_write_refusals_only_retry_when_explicitly_enabled() {
    for enabled in [false, true] {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        Mock::given(method("POST"))
            .respond_with(move |_: &Request| {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    rate_limit(0)
                } else {
                    message()
                }
            })
            .expect(if enabled { 2 } else { 1 })
            .mount(&server)
            .await;
        let result = bot(&server)
            .api()
            .send_message("exact payload".into())
            .chat_id(ChatId::from(1).into())
            .retry_policy(policy().retry_rejected_writes(enabled))
            .await;
        if enabled {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().outcome(), EffectOutcome::Rejected);
        }
        for request in server.received_requests().await.unwrap() {
            assert_eq!(
                request.body_json::<serde_json::Value>().unwrap(),
                json!({"text":"exact payload","chat_id":1})
            );
        }
    }
}

#[tokio::test]
async fn unknown_writes_and_update_acknowledgements_never_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(message().set_delay(Duration::from_secs(5)))
        .expect(2)
        .mount(&server)
        .await;
    let bot = bot(&server);
    let error = bot
        .api()
        .send_message("once".into())
        .chat_id(ChatId::from(1).into())
        .retry_policy(policy().retry_rejected_writes(true))
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let error = bot
        .api()
        .get_updates()
        .offset(10)
        .retry_policy(policy().retry_rejected_writes(true))
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}

#[tokio::test]
async fn read_attempts_and_elapsed_time_are_bounded() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/bot{TOKEN}/getMe")))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;
    let error = bot(&server)
        .api()
        .get_me()
        .retry_policy(policy().max_attempts(2))
        .await
        .unwrap_err();
    assert_eq!(error.http_status(), Some(503));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(identity().set_delay(Duration::from_secs(5)))
        .expect(1)
        .mount(&server)
        .await;
    let started = Instant::now();
    let error = bot(&server)
        .api()
        .get_me()
        .retry_policy(policy().max_elapsed(Duration::from_millis(40)))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[tokio::test]
async fn server_backoff_is_never_shortened_or_slept_beyond_the_budget() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(rate_limit(60))
        .expect(1)
        .mount(&server)
        .await;
    let started = Instant::now();
    let error = bot(&server)
        .api()
        .get_me()
        .retry_policy(policy())
        .await
        .unwrap_err();
    assert_eq!(error.retry_after(), Some(Duration::from_secs(60)));
    assert!(started.elapsed() < Duration::from_millis(500));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(rate_limit(0))
        .expect(1)
        .mount(&server)
        .await;
    let error = bot(&server)
        .api()
        .get_me()
        .retry_policy(
            policy()
                .initial_delay(Duration::from_millis(100))
                .max_delay(Duration::from_secs(1))
                .max_elapsed(Duration::from_millis(50)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Rejected);
}

#[tokio::test]
async fn malformed_permanent_and_migration_errors_are_not_replayed() {
    for response in [
        ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"broken":true}})),
        ResponseTemplate::new(400).set_body_json(
            json!({"ok":false,"error_code":400,"parameters":{"migrate_to_chat_id":-123}}),
        ),
        ResponseTemplate::new(429).set_body_json(
            json!({"ok":false,"error_code":429,"parameters":{"retry_after":0,"migrate_to_chat_id":-123}}),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            bot(&server)
                .api()
                .get_me()
                .retry_policy(policy().retry_rejected_writes(true))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn sensitive_session_and_short_lived_references_are_not_replayed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;
    let client = bot(&server);
    assert!(
        client
            .api()
            .get_managed_bot_token(UserId::from(1))
            .retry_policy(policy().retry_rejected_writes(true))
            .await
            .is_err()
    );
    assert!(
        client
            .api()
            .close()
            .retry_policy(policy().retry_rejected_writes(true))
            .await
            .is_err()
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(rate_limit(0))
        .expect(1)
        .mount(&server)
        .await;
    let params = tg::api::types::EphemeralMessageParameters::new().callback_query_id("cb".into());
    assert!(
        bot(&server)
            .api()
            .send_message("private".into())
            .ephemeral_message_parameters(params)
            .retry_policy(policy().retry_rejected_writes(true))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stream_replay_and_invalid_policy_are_rejected_before_opening_or_dispatch() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    let error = bot
        .api()
        .send_photo("attach://p".into())
        .attach("p", InputFile::path("missing"))
        .retry_policy(policy().retry_rejected_writes(true))
        .await
        .unwrap_err();
    assert_eq!(error.field(), Some("retry_attachments"));
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    let error = bot
        .api()
        .get_me()
        .retry_policy(policy().max_attempts(0))
        .await
        .unwrap_err();
    assert_eq!(error.field(), Some("retry_policy"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn backoff_releases_the_shared_in_flight_permit_and_cancels_cleanly() {
    let server = MockServer::start().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    Mock::given(path(format!("/bot{TOKEN}/getMe")))
        .respond_with(move |_: &Request| {
            signal.notify_one();
            rate_limit(0)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(format!("/bot{TOKEN}/getWebhookInfo"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"url":"","has_custom_certificate":false,"pending_update_count":0}}))).expect(1).mount(&server).await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_in_flight(1)
        .build()
        .unwrap();
    let cloned = bot.clone();
    let task = tokio::spawn(async move {
        cloned
            .api()
            .get_me()
            .retry_policy(
                policy()
                    .initial_delay(Duration::from_millis(500))
                    .max_delay(Duration::from_secs(1)),
            )
            .await
    });
    entered.notified().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    bot.get_webhook_info().await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(30)).await;
}

#[tokio::test]
async fn server_delay_is_respected_even_when_jitter_is_enabled() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                rate_limit(1)
            } else {
                identity()
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let started = Instant::now();
    bot(&server)
        .api()
        .get_me()
        .retry_policy(
            policy()
                .jitter(true)
                .initial_delay(Duration::from_millis(100))
                .max_delay(Duration::from_secs(2))
                .max_elapsed(Duration::from_secs(3)),
        )
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn an_ambiguous_read_timeout_can_recover_with_an_explicit_policy() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                identity().set_delay(Duration::from_secs(5))
            } else {
                identity()
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    bot(&server)
        .api()
        .get_me()
        .retry_policy(policy())
        .await
        .unwrap();
}
