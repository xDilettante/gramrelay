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
    webhook::{
        WebhookError, WebhookFuture, WebhookIngress, WebhookOptions, WebhookSecret, WebhookSink,
    },
};

struct Sink(Arc<AtomicUsize>);
impl WebhookSink for Sink {
    fn ingest(&self, update: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            assert_eq!(update.update_id, 1);
            assert_eq!(update.unknown_fields()["future_event"]["private"], "keep");
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn backend_completion_is_required_and_panics_or_cancellation_release_capacity() {
    use tokio::sync::Notify;
    struct Waiting {
        started: Arc<Notify>,
        release: Arc<Notify>,
        active: Arc<AtomicUsize>,
    }
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl WebhookSink for Waiting {
        fn ingest(&self, _: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
            Box::pin(async move {
                self.active.fetch_add(1, Ordering::SeqCst);
                let _active = Active(self.active.clone());
                self.started.notify_one();
                self.release.notified().await;
                Ok(())
            })
        }
    }
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let ingress = WebhookIngress::new(
        secret(),
        WebhookOptions::default().max_in_flight(1),
        Waiting {
            started: started.clone(),
            release: release.clone(),
            active: active.clone(),
        },
    )
    .unwrap();
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    let task = tokio::spawn(async move { request.deliver(br#"{"update_id":1}"#).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert!(
        !task.is_finished(),
        "a pending backend cannot be acknowledged"
    );
    assert_eq!(
        ingress
            .begin(Some(secret().expose().as_bytes()))
            .unwrap_err(),
        WebhookError::Busy
    );
    release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    let task = tokio::spawn(async move { request.deliver(br#"{"update_id":1}"#).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(active.load(Ordering::SeqCst), 0);
    ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    struct Panicking;
    impl WebhookSink for Panicking {
        fn ingest(&self, _: Update, _deadline: tokio::time::Instant) -> WebhookFuture<'_> {
            panic!("synthetic private panic")
        }
    }
    let ingress = WebhookIngress::new(
        secret(),
        WebhookOptions::default().max_in_flight(1),
        Panicking,
    )
    .unwrap();
    assert_eq!(
        ingress
            .begin(Some(secret().expose().as_bytes()))
            .unwrap()
            .deliver(br#"{"update_id":1}"#)
            .await,
        Err(WebhookError::Handler)
    );
    ingress.begin(Some(secret().expose().as_bytes())).unwrap();
}
fn secret() -> WebhookSecret {
    WebhookSecret::new("SYNTHETIC_SECRET").unwrap()
}

#[tokio::test]
async fn owned_backend_work_receives_the_original_deadline_after_body_wait() {
    struct DeadlineSink(Arc<std::sync::Mutex<Option<tokio::time::Instant>>>);
    impl WebhookSink for DeadlineSink {
        fn ingest(&self, _: Update, deadline: tokio::time::Instant) -> WebhookFuture<'_> {
            Box::pin(async move {
                *self.0.lock().unwrap() = Some(deadline);
                Ok(())
            })
        }
    }
    let observed = Arc::new(std::sync::Mutex::new(None));
    let ingress = WebhookIngress::new(
        secret(),
        WebhookOptions::default().request_timeout(Duration::from_secs(1)),
        DeadlineSink(observed.clone()),
    )
    .unwrap();
    let before = tokio::time::Instant::now();
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    let after = tokio::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(20)).await;
    request.deliver(br#"{"update_id":1}"#).await.unwrap();
    let deadline = observed.lock().unwrap().unwrap();
    assert!(deadline >= before + Duration::from_secs(1));
    assert!(
        deadline <= after + Duration::from_secs(1),
        "reading/queuing must not start a fresh backend deadline"
    );
}
fn ingress(options: WebhookOptions) -> (WebhookIngress, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    (
        WebhookIngress::new(secret(), options, Sink(count.clone())).unwrap(),
        count,
    )
}

#[test]
fn secrets_validate_the_protocol_and_do_not_format_their_values() {
    for value in ["", "space here", "ключ", "a:b", "new\nline"] {
        assert!(WebhookSecret::new(value).is_err());
    }
    assert!(WebhookSecret::new("a".repeat(257)).is_err());
    WebhookSecret::new("a".repeat(256)).unwrap();
    assert!(!format!("{:?}", secret()).contains("SYNTHETIC"));
    assert_eq!(secret().expose(), "SYNTHETIC_SECRET");
}

#[tokio::test]
async fn authentication_and_validation_precede_the_sink_and_preserve_unknown_fields() {
    let (ingress, count) = ingress(WebhookOptions::default());
    for header in [
        None,
        Some(b"wrong".as_slice()),
        Some(b"SYNTHETIC_SECREX".as_slice()),
    ] {
        assert_eq!(
            ingress.begin(header).unwrap_err(),
            WebhookError::Unauthorized
        );
    }
    for body in [
        b"{".as_slice(),
        b"{\"update_id\":-1}".as_slice(),
        b"{\"update_id\":1,\"message\":{}}".as_slice(),
    ] {
        assert_eq!(
            ingress
                .begin(Some(secret().expose().as_bytes()))
                .unwrap()
                .deliver(body)
                .await,
            Err(WebhookError::BadRequest)
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    ingress
        .begin(Some(secret().expose().as_bytes()))
        .unwrap()
        .deliver(br#"{"update_id":1,"future_event":{"private":"keep"}}"#)
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn admission_reserves_event_bytes_and_releases_every_cancelled_request() {
    let (ingress, _) = ingress(
        WebhookOptions::default()
            .max_body_bytes(32)
            .max_buffered_bytes(32)
            .max_in_flight(2),
    );
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    assert_eq!(
        ingress
            .begin(Some(secret().expose().as_bytes()))
            .unwrap_err(),
        WebhookError::Busy
    );
    drop(request);
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    assert_eq!(
        request.deliver(&[b' '; 33]).await,
        Err(WebhookError::PayloadTooLarge)
    );
    ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    ingress.shutdown().await.unwrap();
    assert_eq!(
        ingress
            .begin(Some(secret().expose().as_bytes()))
            .unwrap_err(),
        WebhookError::Closed
    );
}

#[tokio::test]
async fn total_deadline_includes_time_before_delivery_and_shutdown_is_bounded() {
    let (ingress, count) = ingress(
        WebhookOptions::default()
            .request_timeout(Duration::from_millis(10))
            .shutdown_timeout(Duration::from_millis(10)),
    );
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    tokio::time::sleep(Duration::from_millis(15)).await;
    assert_eq!(
        request.deliver(br#"{"update_id":1}"#).await,
        Err(WebhookError::Timeout)
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let request = ingress.begin(Some(secret().expose().as_bytes())).unwrap();
    assert_eq!(ingress.shutdown().await, Err(WebhookError::Timeout));
    assert_eq!(
        request.deliver(br#"{"update_id":1}"#).await,
        Err(WebhookError::Unavailable)
    );
}
