use crate::{Bot, DispatchOptions, EffectOutcome, Error, ErrorKind, Result, Router, Update};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

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
            let updates = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                result = request => result?,
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
