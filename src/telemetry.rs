// Only fixed categories cross this boundary. Never accept request values or errors' Display.
#[cfg(any(feature = "tracing", feature = "metrics"))]
mod enabled {
    use crate::{EffectOutcome, ErrorKind, Result};
    use std::time::Instant;

    pub(crate) struct Attempt {
        method: &'static str,
        started: Instant,
        dispatched: bool,
        finished: bool,
        #[cfg(feature = "tracing")]
        span: tracing::Span,
        #[cfg(feature = "metrics")]
        active: metrics::Gauge,
    }

    impl Attempt {
        pub(crate) fn new(method: &'static str) -> Self {
            #[cfg(feature = "tracing")]
            let span = {
                use std::sync::atomic::{AtomicU64, Ordering};
                static NEXT: AtomicU64 = AtomicU64::new(1);
                tracing::debug_span!(target: "tg", "tg.request", attempt_id = NEXT.fetch_add(1, Ordering::Relaxed), method)
            };
            #[cfg(feature = "metrics")]
            let active = {
                let gauge = metrics::gauge!("tg.request.active", "method" => method);
                gauge.increment(1.0);
                gauge
            };
            Self {
                method,
                started: Instant::now(),
                dispatched: false,
                finished: false,
                #[cfg(feature = "tracing")]
                span,
                #[cfg(feature = "metrics")]
                active,
            }
        }

        pub(crate) fn dispatched(&mut self) {
            self.dispatched = true;
        }

        pub(crate) fn finish<T>(&mut self, result: &Result<T>) {
            self.finished = true;
            match result {
                Ok(_) => self.emit("success", "succeeded", "none"),
                Err(error) => self.emit("error", outcome(error.outcome()), kind(error.kind())),
            }
        }

        fn emit(&self, result: &'static str, outcome: &'static str, error_kind: &'static str) {
            let duration_seconds = self.started.elapsed().as_secs_f64();
            #[cfg(feature = "tracing")]
            tracing::debug!(target: "tg", parent: &self.span, event = "request_finished", method = self.method, result, outcome, error_kind, duration_seconds);
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("tg.request.attempts", "method" => self.method, "result" => result, "outcome" => outcome, "error_kind" => error_kind).increment(1);
                metrics::histogram!("tg.request.duration_seconds", "method" => self.method, "result" => result, "outcome" => outcome, "error_kind" => error_kind).record(duration_seconds);
            }
        }
    }
    impl Drop for Attempt {
        fn drop(&mut self) {
            if !self.finished {
                self.emit(
                    "cancelled",
                    if self.dispatched {
                        "unknown"
                    } else {
                        "not_sent"
                    },
                    "cancelled",
                );
            }
            #[cfg(feature = "metrics")]
            self.active.decrement(1.0);
        }
    }
    fn outcome(value: EffectOutcome) -> &'static str {
        match value {
            EffectOutcome::NotSent => "not_sent",
            EffectOutcome::Rejected => "rejected",
            EffectOutcome::Unknown => "unknown",
        }
    }
    fn kind(value: ErrorKind) -> &'static str {
        match value {
            ErrorKind::Configuration => "configuration",
            ErrorKind::Validation => "validation",
            ErrorKind::Capacity => "capacity",
            ErrorKind::Io => "io",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Transport => "transport",
            ErrorKind::Http => "http",
            ErrorKind::Telegram => "telegram",
            ErrorKind::Decode => "decode",
            ErrorKind::ResponseTooLarge => "response_too_large",
            ErrorKind::Handler => "handler",
        }
    }

    #[cfg(all(feature = "bot", feature = "tracing"))]
    pub(crate) fn polling_reconnect(
        error: ErrorKind,
        consecutive_errors: u32,
        delay: std::time::Duration,
    ) {
        tracing::debug!(
            target: "tg",
            event = "polling_reconnect",
            method = "getUpdates",
            error_kind = kind(error),
            consecutive_errors,
            delay_seconds = delay.as_secs_f64(),
        );
    }
}
#[cfg(any(feature = "tracing", feature = "metrics"))]
pub(crate) use enabled::Attempt;
#[cfg(all(feature = "bot", feature = "tracing"))]
pub(crate) use enabled::polling_reconnect;

#[cfg(not(any(feature = "tracing", feature = "metrics")))]
pub(crate) struct Attempt;
#[cfg(not(any(feature = "tracing", feature = "metrics")))]
impl Attempt {
    pub(crate) fn new(_: &'static str) -> Self {
        Self
    }
    pub(crate) fn dispatched(&mut self) {}
    pub(crate) fn finish<T>(&mut self, _: &crate::Result<T>) {}
}
