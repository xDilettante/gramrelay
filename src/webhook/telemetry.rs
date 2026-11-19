// Only fixed categories/durations cross this boundary. No headers, routes, IDs or bodies.
#[cfg(any(feature = "tracing", feature = "metrics"))]
mod enabled {
    use crate::webhook::WebhookError;
    pub(crate) struct Observation {
        started: std::time::Instant,
        finished: bool,
        #[cfg(feature = "tracing")]
        span: tracing::Span,
        #[cfg(feature = "metrics")]
        active: metrics::Gauge,
    }
    impl Observation {
        pub(crate) fn new() -> Self {
            #[cfg(feature = "tracing")]
            let span = {
                use std::sync::atomic::{AtomicU64, Ordering};
                static NEXT: AtomicU64 = AtomicU64::new(1);
                tracing::debug_span!(target:"tg","tg.webhook",attempt_id=NEXT.fetch_add(1,Ordering::Relaxed))
            };
            #[cfg(feature = "metrics")]
            let active = {
                let active = metrics::gauge!("tg.webhook.active");
                active.increment(1.0);
                active
            };
            Self {
                started: std::time::Instant::now(),
                finished: false,
                #[cfg(feature = "tracing")]
                span,
                #[cfg(feature = "metrics")]
                active,
            }
        }
        #[cfg(feature = "tracing")]
        pub(crate) fn span(&self) -> tracing::Span {
            self.span.clone()
        }
        pub(crate) fn finish(&mut self, result: &Result<(), WebhookError>) {
            self.finished = true;
            match result {
                Ok(()) => self.emit("accepted", "200", "none"),
                Err(error) => self.emit("refused", status(error.status()), error.code()),
            }
        }
        fn emit(&self, result: &'static str, status: &'static str, reason: &'static str) {
            let duration_seconds = self.started.elapsed().as_secs_f64();
            #[cfg(feature = "tracing")]
            tracing::debug!(target:"tg",parent:&self.span,event="webhook_finished",result,status,reason,duration_seconds);
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("tg.webhook.requests","result"=>result,"status"=>status,"reason"=>reason).increment(1);
                metrics::histogram!("tg.webhook.duration_seconds","result"=>result,"status"=>status,"reason"=>reason).record(duration_seconds);
            }
        }
    }
    impl Drop for Observation {
        fn drop(&mut self) {
            if !self.finished {
                self.emit("cancelled", "none", "cancelled");
            }
            #[cfg(feature = "metrics")]
            self.active.decrement(1.0);
        }
    }
    fn status(value: u16) -> &'static str {
        match value {
            400 => "400",
            401 => "401",
            409 => "409",
            413 => "413",
            415 => "415",
            500 => "500",
            503 => "503",
            504 => "504",
            _ => "none",
        }
    }
}
#[cfg(any(feature = "tracing", feature = "metrics"))]
pub(crate) use enabled::Observation;
#[cfg(not(any(feature = "tracing", feature = "metrics")))]
pub(crate) struct Observation;
#[cfg(not(any(feature = "tracing", feature = "metrics")))]
impl Observation {
    pub(crate) fn new() -> Self {
        Self
    }
    pub(crate) fn finish(&mut self, _: &Result<(), crate::webhook::WebhookError>) {}
}
