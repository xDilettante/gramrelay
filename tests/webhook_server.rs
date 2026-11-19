#![cfg(feature = "webhook")]
use std::{sync::Arc, time::Duration};
use tg::{
    CancellationToken, Update,
    webhook::{WebhookFuture, WebhookIngress, WebhookOptions, WebhookSecret, WebhookSink},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};
const TOKEN: &str = "SYNTHETIC_SECRET";
struct Waiting(Arc<Notify>, Arc<Notify>);
impl WebhookSink for Waiting {
    fn ingest(&self, _: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            self.0.notify_one();
            self.1.notified().await;
            Ok(())
        })
    }
}
struct Immediate;
impl WebhookSink for Immediate {
    fn ingest(&self, _: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}
fn ingress(options: WebhookOptions, sink: impl WebhookSink) -> WebhookIngress {
    WebhookIngress::new(WebhookSecret::new(TOKEN).unwrap(), options, sink).unwrap()
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
async fn post(client: &reqwest::Client, url: &str) -> reqwest::Result<reqwest::Response> {
    client
        .post(url)
        .header("x-telegram-bot-api-secret-token", TOKEN)
        .header("content-type", "application/json")
        .body(r#"{"update_id":1}"#)
        .send()
        .await
}

#[tokio::test]
async fn connection_limit_and_header_timeout_bound_slow_clients() {
    let ingress = ingress(
        WebhookOptions::default()
            .max_connections(1)
            .header_timeout(Duration::from_millis(150)),
        Immediate,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let server = {
        let stop = stop.clone();
        tokio::spawn(async move { ingress.serve_until(listener, stop).await })
    };
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    slow.write_all(b"POST /telegram HTTP/1.1\r\n")
        .await
        .unwrap();
    let client = client();
    let url = format!("http://{address}/telegram");
    let next = post(&client, &url);
    tokio::pin!(next);
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut next)
            .await
            .is_err(),
        "another socket must wait behind the admitted slow connection"
    );
    assert_eq!(next.await.unwrap().status(), 200);
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), slow.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn graceful_stop_waits_for_backend_before_responding_and_closes_admission() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ingress = ingress(
        WebhookOptions::default(),
        Waiting(started.clone(), release.clone()),
    );
    let control = ingress.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/telegram", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let server = {
        let stop = stop.clone();
        tokio::spawn(async move { ingress.serve_until(listener, stop).await })
    };
    let request = tokio::spawn(async move { post(&client(), &url).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert!(!request.is_finished());
    stop.cancel();
    release.notify_one();
    assert_eq!(request.await.unwrap().unwrap().status(), 200);
    server.await.unwrap().unwrap();
    assert_eq!(
        control.begin(Some(TOKEN.as_bytes())).unwrap_err(),
        tg::webhook::WebhookError::Closed
    );
}

#[tokio::test]
async fn forced_stop_aborts_owned_connections_and_reports_the_unconfirmed_work() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ingress = ingress(
        WebhookOptions::default().shutdown_timeout(Duration::from_millis(20)),
        Waiting(started.clone(), release),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/telegram", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let server = {
        let stop = stop.clone();
        tokio::spawn(async move { ingress.serve_until(listener, stop).await })
    };
    let request = tokio::spawn(async move { post(&client(), &url).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    stop.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), tg::ErrorKind::Timeout);
    assert_eq!(error.outcome(), tg::EffectOutcome::Unknown);
    assert!(request.await.unwrap().is_err());
}

#[tokio::test]
async fn cancelling_the_whole_server_future_closes_owned_requests_and_admission() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ingress = ingress(WebhookOptions::default(), Waiting(started.clone(), release));
    let control = ingress.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/telegram", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        ingress
            .serve_until(listener, CancellationToken::new())
            .await
    });
    let request = tokio::spawn(async move { post(&client(), &url).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(
        control.begin(Some(TOKEN.as_bytes())).unwrap_err(),
        tg::webhook::WebhookError::Closed
    );
}
