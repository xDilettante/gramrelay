#![cfg(all(feature = "webhook", feature = "tracing", feature = "metrics"))]
#[path = "support/telemetry_capture.rs"]
mod telemetry_capture;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use telemetry_capture::{Capture, Output};
use tg::{
    CancellationToken, Update,
    webhook::{
        WebhookError, WebhookFuture, WebhookIngress, WebhookOptions, WebhookSecret, WebhookSink,
    },
};
use tokio::sync::Notify;
use tracing_subscriber::prelude::*;
const SECRET: &str = "PRIVATE_WEBHOOK_SECRET";
struct Sink(Arc<Notify>);
impl WebhookSink for Sink {
    fn ingest(&self, update: Update, _: tokio::time::Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            tracing::debug!(target:"tg",event="sink_seen");
            match update.update_id {
                1 => Ok(()),
                2 => Err(WebhookError::Unavailable),
                5 => panic!("PRIVATE_PANIC"),
                _ => {
                    self.0.notify_one();
                    std::future::pending().await
                }
            }
        })
    }
}
fn event(id: i64) -> Value {
    json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":99887766,"type":"private"},"text":"PRIVATE_TEXT"},"future":{"path":"PRIVATE_FILE","callback":"PRIVATE_CALLBACK"}})
}

#[test]
fn incoming_decisions_are_private_bounded_and_balance_cancelled_http_lifetimes() {
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(move || writer.clone())
            .with_filter(
                tracing_subscriber::filter::Targets::new().with_target("tg", tracing::Level::DEBUG),
            ),
    );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let capture = Capture::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    metrics::with_local_recorder(&capture, || {
        runtime.block_on(async {
            let started = Arc::new(Notify::new());
            let options = WebhookOptions::default()
                .path("/PRIVATE_ROUTE")
                .max_body_bytes(1024)
                .max_buffered_bytes(1024)
                .max_in_flight(1)
                .request_timeout(Duration::from_millis(100))
                .shutdown_timeout(Duration::from_millis(20));
            let ingress = WebhookIngress::new(
                WebhookSecret::new(SECRET).unwrap(),
                options,
                Sink(started.clone()),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/PRIVATE_ROUTE", listener.local_addr().unwrap());
            let stop = CancellationToken::new();
            let server = {
                let stop = stop.clone();
                tokio::spawn(async move { ingress.serve_until(listener, stop).await })
            };
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            for (id, expected) in [(1, 200), (2, 503), (5, 500)] {
                assert_eq!(
                    client
                        .post(&url)
                        .header("x-telegram-bot-api-secret-token", SECRET)
                        .json(&event(id))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    expected
                );
            }
            assert_eq!(
                client
                    .post(&url)
                    .json(&event(1))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
            assert_eq!(
                client
                    .post(&url)
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .header("content-type", "application/json")
                    .body("{")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
            assert_eq!(
                client
                    .post(&url)
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .header("content-type", "text/plain")
                    .body("PRIVATE_BAD_TYPE")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                415
            );
            assert_eq!(
                client
                    .post(&url)
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .header("content-type", "application/json")
                    .body("x".repeat(1025))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                413
            );
            let request = {
                let client = client.clone();
                let url = url.clone();
                tokio::spawn(async move {
                    client
                        .post(url)
                        .header("x-telegram-bot-api-secret-token", SECRET)
                        .json(&event(3))
                        .send()
                        .await
                        .unwrap()
                })
            };
            tokio::time::timeout(Duration::from_secs(2), started.notified())
                .await
                .unwrap();
            assert_eq!(
                client
                    .post(&url)
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .json(&event(1))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                503
            );
            assert_eq!(request.await.unwrap().status(), 504);
            let request = tokio::spawn(async move {
                client
                    .post(url)
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .json(&event(4))
                    .send()
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), started.notified())
                .await
                .unwrap();
            stop.cancel();
            assert_eq!(
                server.await.unwrap().unwrap_err().kind(),
                tg::ErrorKind::Timeout
            );
            assert!(request.await.unwrap().is_err());
        })
    });
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!text.contains("PRIVATE_"));
    assert!(!text.contains("99887766"));
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let finished: Vec<_> = events
        .iter()
        .filter(|e| e["fields"]["event"] == "webhook_finished")
        .collect();
    assert_eq!(finished.len(), 10);
    assert!(finished.iter().all(|e| e["span"]["attempt_id"].is_u64()));
    assert!(
        events
            .iter()
            .filter(|e| e["fields"]["event"] == "sink_seen")
            .all(|e| e["span"]["name"] == "tg.webhook")
    );
    let samples = capture.0.lock().unwrap();
    assert!(!format!("{samples:?}").contains("PRIVATE_"));
    assert_eq!(
        samples
            .iter()
            .filter(|(k, _)| k.name() == "tg.webhook.requests")
            .count(),
        10
    );
    assert_eq!(
        samples
            .iter()
            .filter(|(k, v)| k.name() == "tg.webhook.duration_seconds"
                && v.is_finite()
                && *v >= 0.0)
            .count(),
        10
    );
    assert_eq!(
        samples
            .iter()
            .filter(|(k, _)| k.name() == "tg.webhook.active")
            .map(|(_, v)| v)
            .sum::<f64>(),
        0.0
    );
    let mut cancelled = false;
    for (key, _) in samples.iter() {
        assert!(key.name().starts_with("tg.webhook."));
        for label in key.labels() {
            assert!(matches!(label.key(), "result" | "status" | "reason"));
            assert!(!label.value().contains("PRIVATE_"));
            match label.key() {
                "result" => assert!(matches!(
                    label.value(),
                    "accepted" | "refused" | "cancelled"
                )),
                "status" => assert!(matches!(
                    label.value(),
                    "none" | "200" | "400" | "401" | "409" | "413" | "415" | "500" | "503" | "504"
                )),
                "reason" => assert!(matches!(
                    label.value(),
                    "none"
                        | "cancelled"
                        | "webhook_bad_request"
                        | "webhook_unauthorized"
                        | "webhook_payload_too_large"
                        | "webhook_media_type"
                        | "webhook_conflict"
                        | "webhook_busy"
                        | "webhook_closed"
                        | "webhook_unavailable"
                        | "webhook_timeout"
                        | "webhook_handler"
                )),
                _ => unreachable!(),
            }
            if label.key() == "result" && label.value() == "cancelled" {
                cancelled = true;
            }
        }
    }
    assert!(cancelled);
}
