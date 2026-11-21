use crate::{Bot, DispatchOptions, EffectOutcome, Error, ErrorKind, Result, Router, Update};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// Reconnect settings for the managed `getUpdates` loop.
///
/// Enable with [`PollingOptions::reconnect`]. The default backoff grows from one
/// second to thirty seconds, with jitter. The tenth consecutive failure ends
/// polling unless the limit is changed.
///
/// Startup checks and replies that exceed the configured body limit return their
/// errors immediately.
#[derive(Debug, Clone)]
pub struct PollingReconnect {
    initial_delay: Duration,
    max_delay: Duration,
    max_consecutive_errors: Option<u32>,
}

impl Default for PollingReconnect {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            max_consecutive_errors: Some(10),
        }
    }
}

impl PollingReconnect {
    /// Set the initial backoff before jitter. Must be greater than zero.
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Cap the local backoff. Telegram's `retry_after` may require a longer wait.
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Stop on this many consecutive failures. `None` keeps trying until
    /// cancellation or a permanent error. A successful poll resets the count.
    pub fn max_consecutive_errors(mut self, limit: Option<u32>) -> Self {
        self.max_consecutive_errors = limit;
        self
    }

    fn validate(&self) -> Result<()> {
        if self.initial_delay.is_zero() || self.initial_delay > self.max_delay {
            return Err(Error::local(
                ErrorKind::Validation,
                "reconnect_initial_delay",
            ));
        }
        if Instant::now().checked_add(self.max_delay).is_none() {
            return Err(Error::local(ErrorKind::Validation, "reconnect_max_delay"));
        }
        if self.max_consecutive_errors == Some(0) {
            return Err(Error::local(
                ErrorKind::Validation,
                "reconnect_max_consecutive_errors",
            ));
        }
        Ok(())
    }
}

/// Bounds for the in-memory polling runtime. Durable recovery is a separate integration.
#[derive(Debug, Clone)]
pub struct PollingOptions {
    batch_limit: u8,
    max_concurrency: usize,
    poll_timeout: u32,
    handler_timeout: Duration,
    shutdown_timeout: Duration,
    empty_poll_delay: Duration,
    start_offset: Option<i64>,
    allowed_updates: Vec<String>,
    reconnect: Option<PollingReconnect>,
}
impl Default for PollingOptions {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            max_concurrency: 16,
            poll_timeout: 20,
            handler_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            empty_poll_delay: Duration::from_millis(250),
            start_offset: None,
            allowed_updates: vec!["message".into(), "callback_query".into()],
            reconnect: None,
        }
    }
}
impl PollingOptions {
    pub fn batch_limit(mut self, limit: u8) -> Self {
        self.batch_limit = limit;
        self
    }
    pub fn max_concurrency(mut self, limit: usize) -> Self {
        self.max_concurrency = limit;
        self
    }
    /// Telegram's long-poll timeout in seconds, below the client's overall request deadline.
    pub fn poll_timeout(mut self, seconds: u32) -> Self {
        self.poll_timeout = seconds;
        self
    }
    pub fn handler_timeout(mut self, timeout: Duration) -> Self {
        self.handler_timeout = timeout;
        self
    }
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }
    pub fn empty_poll_delay(mut self, delay: Duration) -> Self {
        self.empty_poll_delay = delay;
        self
    }
    /// Only supply a checkpoint whose prior updates were handled or durably stored.
    pub fn start_offset(mut self, offset: i64) -> Self {
        self.start_offset = Some(offset);
        self
    }
    pub fn allowed_updates(mut self, kinds: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_updates = kinds.into_iter().map(Into::into).collect();
        self
    }

    /// Retry transient `getUpdates` failures with the same offset. By default,
    /// the managed loop returns each failure to the caller immediately.
    pub fn reconnect(mut self, policy: PollingReconnect) -> Self {
        self.reconnect = Some(policy);
        self
    }

    fn validate(&self, bot: &Bot) -> Result<()> {
        if !(1..=100).contains(&self.batch_limit) {
            return Err(Error::local(ErrorKind::Validation, "batch_limit"));
        }
        if !(1..=100).contains(&self.max_concurrency) {
            return Err(Error::local(ErrorKind::Validation, "max_concurrency"));
        }
        if self.start_offset.is_some_and(|id| id < 0) {
            return Err(Error::local(ErrorKind::Validation, "offset"));
        }
        if Duration::from_secs(self.poll_timeout.into()) >= bot.request_deadline() {
            return Err(Error::local(ErrorKind::Validation, "poll_timeout"));
        }
        for (field, timeout) in [
            ("handler_timeout", self.handler_timeout),
            ("shutdown_timeout", self.shutdown_timeout),
            ("empty_poll_delay", self.empty_poll_delay),
        ] {
            if timeout.is_zero() || Instant::now().checked_add(timeout).is_none() {
                return Err(Error::local(ErrorKind::Validation, field));
            }
        }
        if let Some(reconnect) = &self.reconnect {
            reconnect.validate()?;
        }
        Ok(())
    }
}

impl Bot {
    /// Run until Ctrl-C, then drain accepted work within the default shutdown deadline.
    /// Polling owns a process-wide lease for this bot ID.
    pub async fn run_polling<S: Send + Sync + 'static>(&self, router: Router<S>) -> Result<()> {
        let stop = CancellationToken::new();
        let run = self.run_polling_until(router, PollingOptions::default(), stop.clone());
        tokio::pin!(run);
        tokio::select! {
            result = &mut run => result,
            signal = tokio::signal::ctrl_c() => {
                stop.cancel();
                let result = run.await;
                signal.map_err(|_| Error::local(ErrorKind::Configuration, "shutdown_signal"))?;
                result
            }
        }
    }

    /// Run with explicit cancellation. Handler failure leaves the current batch unacknowledged.
    pub async fn run_polling_until<S: Send + Sync + 'static>(
        &self,
        mut router: Router<S>,
        options: PollingOptions,
        stop: CancellationToken,
    ) -> Result<()> {
        options.validate(self)?;
        router.validate()?;
        if stop.is_cancelled() {
            return Ok(());
        }
        let _owner = self.claim_update_source()?;
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            result = self.prepare_router(&mut router) => result?,
        };
        let webhook = tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            result = self.get_webhook_info() => result?,
        };
        if !webhook.url.is_empty() {
            return Err(Error::local(ErrorKind::Configuration, "webhook_active"));
        }

        let router = Arc::new(router);
        let mut offset = options.start_offset;
        let mut consecutive_errors = 0_u32;
        let mut backoff = Duration::ZERO;
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            let mut request = self
                .get_updates()
                .limit(options.batch_limit)
                .timeout(options.poll_timeout)
                .allowed_updates(options.allowed_updates.iter().cloned());
            if let Some(offset) = offset {
                request = request.offset(offset);
            }
            let received = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                result = request => result,
            };
            let updates = match received {
                Ok(updates) => {
                    consecutive_errors = 0;
                    backoff = Duration::ZERO;
                    updates
                }
                Err(error) => {
                    let Some(policy) = &options.reconnect else {
                        return Err(error);
                    };
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    if !transient_poll_error(&error)
                        || policy
                            .max_consecutive_errors
                            .is_some_and(|limit| consecutive_errors >= limit)
                    {
                        return Err(error);
                    }
                    backoff = backoff
                        .saturating_mul(2)
                        .clamp(policy.initial_delay, policy.max_delay);
                    let jittered = backoff.mul_f64(0.5 + 0.5 * fastrand::f64());
                    let delay = jittered.max(error.retry_after().unwrap_or_default());
                    let Some(wake) = tokio::time::Instant::now().checked_add(delay) else {
                        return Err(error);
                    };
                    #[cfg(feature = "tracing")]
                    crate::telemetry::polling_reconnect(error.kind(), consecutive_errors, delay);
                    // Only the receive is retried. At this point the current offset
                    // acknowledges at most the last fully processed batch.
                    tokio::select! {
                        _ = stop.cancelled() => return Ok(()),
                        _ = tokio::time::sleep_until(wake) => {}
                    }
                    continue;
                }
            };
            let next = next_offset(&updates, offset, options.batch_limit)?;
            if updates.is_empty() {
                tokio::select! {
                    _ = stop.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(options.empty_poll_delay) => {}
                }
                continue;
            }
            let dispatch = DispatchOptions::default()
                .max_concurrency(options.max_concurrency)
                .handler_timeout(options.handler_timeout);
            let work = self.dispatch_batch(router.clone(), updates, &dispatch);
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => result?,
                _ = stop.cancelled() => {
                    return tokio::time::timeout(options.shutdown_timeout, &mut work).await
                        .map_err(|_| Error::new(ErrorKind::Timeout, EffectOutcome::Unknown))?;
                }
            }
            // Merely setting this local cursor does not acknowledge anything.
            // Only a subsequent getUpdates request can acknowledge the completed batch.
            offset = next;
        }
    }
}

fn transient_poll_error(error: &Error) -> bool {
    match error.kind() {
        ErrorKind::Timeout | ErrorKind::Transport => true,
        ErrorKind::Http => error
            .http_status()
            .is_some_and(|status| (500..600).contains(&status)),
        ErrorKind::Telegram => {
            error.api_error_code() == Some(429)
                && error
                    .http_status()
                    .is_some_and(|status| status == 429 || (200..300).contains(&status))
        }
        _ => false,
    }
}

fn next_offset(updates: &[Update], previous: Option<i64>, limit: u8) -> Result<Option<i64>> {
    let invalid = updates.len() > usize::from(limit)
        || updates.iter().any(|u| u.update_id < 0)
        || updates
            .windows(2)
            .any(|pair| pair[0].update_id >= pair[1].update_id)
        || updates
            .first()
            .is_some_and(|u| previous.is_some_and(|offset| u.update_id < offset));
    if invalid {
        return Err(Error::new(ErrorKind::Decode, EffectOutcome::Unknown));
    }
    match updates.last() {
        Some(last) => last
            .update_id
            .checked_add(1)
            .map(Some)
            .ok_or_else(|| Error::new(ErrorKind::Decode, EffectOutcome::Unknown)),
        None => Ok(previous),
    }
}
