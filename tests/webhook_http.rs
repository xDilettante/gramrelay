#![cfg(feature = "webhook")]
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg::{
    Update,
    webhook::{WebhookFuture, WebhookIngress, WebhookOptions, WebhookSecret},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const SECRET: &str = "SYNTHETIC_WEBHOOK_SECRET";
const HEADER: &str = "x-telegram-bot-api-secret-token";
struct Sink(Arc<AtomicUsize>);
impl tg::webhook::WebhookSink for Sink {
    fn ingest(&self, update: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            assert_eq!(update.update_id, 1);
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
struct Server {
    url: String,
    stop: tg::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    ingress: WebhookIngress,
    count: Arc<AtomicUsize>,
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
    async fn new(options: WebhookOptions) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let ingress = WebhookIngress::new(
            WebhookSecret::new(SECRET).unwrap(),
            options,
            Sink(count.clone()),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = tg::CancellationToken::new();
        let signal = stop.clone();
        let router = ingress.router();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(signal.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            url,
            stop,
            task: Some(task),
            ingress,
            count,
        }
    }
    async fn raw(&self, headers: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(self.url.trim_start_matches("http://"))
            .await
            .unwrap();
        stream.write_all(format!("POST /telegram HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }
    async fn finish(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

#[tokio::test]
async fn rejects_unauthenticated_or_oversized_headers_without_waiting_for_a_body() {
    let server = Server::new(WebhookOptions::default().max_body_bytes(64)).await;
    assert!(
        server
            .raw("Content-Type: application/json\r\nContent-Length: 999999\r\n")
            .await
            .starts_with("HTTP/1.1 401")
    );
    assert!(server.raw(&format!("{HEADER}: {SECRET}\r\n{HEADER}: {SECRET}\r\nContent-Type: application/json\r\nContent-Length: 1\r\n")).await.starts_with("HTTP/1.1 401"));
    assert!(
        server
            .raw(&format!(
                "{HEADER}: {SECRET}\r\nContent-Type: application/json\r\nContent-Length: 65\r\n"
            ))
            .await
            .starts_with("HTTP/1.1 413")
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    server.finish().await;
}

#[tokio::test]
async fn accepts_real_json_and_rejects_content_type_encoding_and_chunked_overflow() {
    let server = Server::new(WebhookOptions::default().max_body_bytes(32)).await;
    let client = client();
    let url = format!("{}/telegram", server.url);
    let response = client
        .post(&url)
        .header(HEADER, SECRET)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(r#"{"update_id":1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    for (field, value) in [("Content-Type", "text/plain"), ("Content-Encoding", "gzip")] {
        let response = client
            .post(&url)
            .header(HEADER, SECRET)
            .header("Content-Type", "application/json")
            .header(field, value)
            .body(r#"{"update_id":1}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 415);
    }
    let stream =
        futures_util::stream::iter([Ok::<_, std::io::Error>(vec![b' '; 16]), Ok(vec![b' '; 17])]);
    let response = client
        .post(&url)
        .header(HEADER, SECRET)
        .header("Content-Type", "application/json")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    server.finish().await;
}

#[tokio::test]
async fn a_stalled_body_hits_the_whole_request_deadline_and_releases_admission() {
    let server = Server::new(
        WebhookOptions::default()
            .request_timeout(Duration::from_millis(20))
            .max_in_flight(1),
    )
    .await;
    let response = server
        .raw(&format!(
            "{HEADER}: {SECRET}\r\nContent-Type: application/json\r\nContent-Length: 10\r\n"
        ))
        .await;
    assert!(response.starts_with("HTTP/1.1 504"));
    assert!(!response.contains(SECRET));
    let response = client()
        .post(format!("{}/telegram", server.url))
        .header(HEADER, SECRET)
        .header("Content-Type", "application/json")
        .body(r#"{"update_id":1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    server.ingress.shutdown().await.unwrap();
    let response = client()
        .post(format!("{}/telegram", server.url))
        .header(HEADER, SECRET)
        .header("Content-Type", "application/json")
        .body(r#"{"update_id":1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    server.finish().await;
}
