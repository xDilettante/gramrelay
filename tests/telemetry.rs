#![cfg(all(feature = "tracing", feature = "metrics"))]

use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tg::{Bot, ChatId, ErrorKind, InputFile};
use tracing::Instrument;
use tracing_subscriber::prelude::*;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Capture(Arc<Mutex<Vec<(Key, f64)>>>);
struct Sample {
    key: Key,
    samples: Arc<Mutex<Vec<(Key, f64)>>>,
}
impl Sample {
    fn record(&self, value: f64) {
        self.samples.lock().unwrap().push((self.key.clone(), value));
    }
}
impl CounterFn for Sample {
    fn increment(&self, value: u64) {
        self.record(value as f64);
    }
    fn absolute(&self, value: u64) {
        self.record(value as f64);
    }
}
impl GaugeFn for Sample {
    fn increment(&self, value: f64) {
        self.record(value);
    }
    fn decrement(&self, value: f64) {
        self.record(-value);
    }
    fn set(&self, value: f64) {
        self.record(value);
    }
}
impl HistogramFn for Sample {
    fn record(&self, value: f64) {
        self.record(value);
    }
}
impl Capture {
    fn handle(&self, key: &Key) -> Arc<Sample> {
        Arc::new(Sample {
            key: key.clone(),
            samples: self.0.clone(),
        })
    }
}
impl Recorder for Capture {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        Counter::from_arc(self.handle(key))
    }
    fn register_gauge(&self, key: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(self.handle(key))
    }
    fn register_histogram(&self, key: &Key, _: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(self.handle(key))
    }
}

#[test]
fn attempts_are_observable_and_private_including_capacity_and_cancellation() {
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
        let server = MockServer::start().await;
        let bot = Bot::builder("123456:PRIVATE_TOKEN").api_url(server.uri())
            .allow_http_loopback(true).max_in_flight(1).build().unwrap();
        Mock::given(path("/bot123456:PRIVATE_TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"id":123456,"is_bot":true,"first_name":"PRIVATE_NAME"}})))
            .mount(&server).await;
        bot.get_me().instrument(tracing::info_span!(target: "tg", "host.request")).await.unwrap();
        server.reset().await;
        Mock::given(path("/bot123456:PRIVATE_TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"ok":false,"error_code":429,"description":"PRIVATE_REMOTE_DESCRIPTION","parameters":{"retry_after":9}})))
            .mount(&server).await;
        bot.send_message(ChatId::from(99887766), "PRIVATE_TEXT").await.unwrap_err();
        Mock::given(path("/file/bot123456:PRIVATE_TOKEN/documents/PRIVATE_PATH"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PRIVATE_BYTES"))
            .mount(&server).await;
        let file: tg::api::types::File = serde_json::from_value(json!({"file_id":"PRIVATE_FILE_ID","file_unique_id":"PRIVATE_UNIQUE","file_path":"documents/PRIVATE_PATH"})).unwrap();
        assert_eq!(bot.download_file(&file).bytes().await.unwrap(), b"PRIVATE_BYTES");
        bot.send_document(ChatId::from(99887766), InputFile::path("PRIVATE_MISSING_FILE")).await.unwrap_err();
        Mock::given(path("/bot123456:PRIVATE_TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server).await;
        let mut pending = Box::pin(bot.get_me());
        tokio::select! {
            result = &mut pending => panic!("unexpected response: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        assert_eq!(bot.get_me().await.unwrap_err().kind(), ErrorKind::Capacity);
        drop(pending);
    })
    });
    let log = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!log.contains("PRIVATE_"), "private diagnostics: {log}");
    assert!(!log.contains("99887766"));
    let events: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let complete: Vec<_> = events
        .iter()
        .filter(|event| event["fields"]["event"] == "request_finished")
        .collect();
    assert_eq!(complete.len(), 6, "{log}");
    assert!(
        complete[0]["spans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|span| span["name"] == "host.request")
    );
    assert!(
        complete
            .iter()
            .any(|event| event["fields"]["result"] == "cancelled"
                && event["fields"]["outcome"] == "unknown")
    );
    assert!(
        complete
            .iter()
            .any(|event| event["fields"]["error_kind"] == "capacity"
                && event["fields"]["outcome"] == "not_sent")
    );
    assert!(
        complete
            .iter()
            .all(|event| event["span"]["attempt_id"].is_u64())
    );
    let samples = capture.0.lock().unwrap();
    assert!(!format!("{samples:?}").contains("PRIVATE_"));
    assert_eq!(
        samples
            .iter()
            .filter(|(key, _)| key.name() == "tg.request.attempts")
            .count(),
        6
    );
    assert_eq!(
        samples
            .iter()
            .filter(|(key, value)| key.name() == "tg.request.duration_seconds"
                && value.is_finite()
                && *value >= 0.0)
            .count(),
        6
    );
    let mut gauges = BTreeMap::new();
    for (key, value) in samples
        .iter()
        .filter(|(key, _)| key.name() == "tg.request.active")
    {
        *gauges.entry(format!("{key:?}")).or_insert(0.0) += value;
    }
    assert!(!gauges.is_empty());
    assert!(gauges.values().all(|value| *value == 0.0), "{gauges:?}");
    for (key, _) in samples.iter() {
        assert!(
            key.labels()
                .all(|label| matches!(label.key(), "method" | "result" | "outcome" | "error_kind"))
        );
    }
}
