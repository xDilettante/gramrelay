use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
#[derive(Clone, Default)]
pub(super) struct Output(pub Arc<Mutex<Vec<u8>>>);
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
pub(super) struct Capture(pub Arc<Mutex<Vec<(Key, f64)>>>);
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
