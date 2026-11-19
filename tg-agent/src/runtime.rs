//! Managed durable intake and at-least-once handlers. Enable the `bot` feature.
use crate::{
    journal::{JournalError, JournalErrorKind, now, types::millis},
    updates::{ConsumerLease, SourceLease, SourceMode, UpdateFeed},
};
use std::{fmt, sync::Arc, time::Duration};
use tg::{Bot, CancellationToken, DispatchOptions, RetryPolicy, Router, api::methods::GetUpdates};
use tokio::time::Instant;

/// Safe diagnostics. An error never grants permission to repeat an external handler effect.
#[derive(Debug)]
#[non_exhaustive]
pub enum RuntimeError {
    Journal(JournalError),
    Telegram(tg::Error),
    Configuration,
    ShutdownTimeout,
}
impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => fmt::Display::fmt(error, f),
            Self::Telegram(error) => fmt::Display::fmt(error, f),
            Self::Configuration => f.write_str("runtime_configuration"),
            Self::ShutdownTimeout => f.write_str("runtime_shutdown_timeout"),
        }
    }
}
impl std::error::Error for RuntimeError {}
impl From<JournalError> for RuntimeError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}
impl From<tg::Error> for RuntimeError {
    fn from(error: tg::Error) -> Self {
        Self::Telegram(error)
    }
}
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// No retry is implicit. Restarting intake loads its last committed acknowledgement offset.
#[derive(Debug, Clone)]
pub struct PollOptions {
    batch_limit: u8,
    poll_timeout: u32,
    request_budget: Duration,
    empty_poll_delay: Duration,
    allowed_updates: Vec<String>,
}
impl Default for PollOptions {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            poll_timeout: 20,
            request_budget: Duration::from_secs(30),
            empty_poll_delay: Duration::from_millis(250),
            allowed_updates: vec!["message".into(), "callback_query".into()],
        }
    }
}
impl PollOptions {
    pub fn batch_limit(mut self, value: u8) -> Self {
        self.batch_limit = value;
        self
    }
    pub fn poll_timeout(mut self, value: u32) -> Self {
        self.poll_timeout = value;
        self
    }
    pub fn request_budget(mut self, value: Duration) -> Self {
        self.request_budget = value;
        self
    }
    pub fn empty_poll_delay(mut self, value: Duration) -> Self {
        self.empty_poll_delay = value;
        self
    }
    pub fn allowed_updates(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_updates = values.into_iter().map(Into::into).collect();
        self
    }
    fn validate(&self, lease: Duration) -> Result<()> {
        if !(1..=100).contains(&self.batch_limit)
            || !valid_duration(self.request_budget)
            || !valid_duration(self.empty_poll_delay)
            || self.request_budget > lease / 2
            || Duration::from_secs(self.poll_timeout.into()) >= self.request_budget
            || self.empty_poll_delay >= lease / 3
        {
            return Err(RuntimeError::Configuration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ConsumerOptions {
    batch_limit: usize,
    dispatch: DispatchOptions,
    idle_delay: Duration,
    shutdown_timeout: Duration,
}
impl Default for ConsumerOptions {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            dispatch: DispatchOptions::default(),
            idle_delay: Duration::from_millis(250),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}
impl ConsumerOptions {
    pub fn batch_limit(mut self, value: usize) -> Self {
        self.batch_limit = value;
        self
    }
    pub fn dispatch(mut self, value: DispatchOptions) -> Self {
        self.dispatch = value;
        self
    }
    pub fn idle_delay(mut self, value: Duration) -> Self {
        self.idle_delay = value;
        self
    }
    pub fn shutdown_timeout(mut self, value: Duration) -> Self {
        self.shutdown_timeout = value;
        self
    }
    fn validate(&self, lease: Duration) -> Result<()> {
        self.dispatch.validate()?;
        if !(1..=100).contains(&self.batch_limit)
            || lease / 3 < Duration::from_millis(1)
            || !valid_duration(self.idle_delay)
            || self.idle_delay >= lease / 3
            || !valid_duration(self.shutdown_timeout)
        {
            return Err(RuntimeError::Configuration);
        }
        Ok(())
    }
}
fn valid_duration(value: Duration) -> bool {
    !value.is_zero() && Instant::now().checked_add(value).is_some()
}

impl UpdateFeed {
    /// Commit intake before sending another getUpdates offset. Refuses an active webhook.
    /// Token cancellation releases ownership after admitted DB work drains. Dropping the
    /// entire future/process instead leaves a bounded database lease for recovery.
    pub async fn run_polling_until(
        &self,
        bot: &Bot,
        options: PollOptions,
        stop: CancellationToken,
    ) -> Result<()> {
        options.validate(self.lease_duration())?;
        if stop.is_cancelled() {
            return Ok(());
        }
        let _guard = bot.claim_update_source()?;
        let identity = tokio::select! { _=stop.cancelled()=>return Ok(()), result=bot.verified_identity()=>result? };
        if i64::from(identity.id) != self.bot_id() {
            return Err(RuntimeError::Configuration);
        }
        let webhook = tokio::select! { _=stop.cancelled()=>return Ok(()), result=bot.get_webhook_info()=>result? };
        if !webhook.url.is_empty() {
            return Err(RuntimeError::Configuration);
        }
        let mut lease = self.claim_source(SourceMode::Polling).await?;
        let result = tokio::select! {
            _=stop.cancelled()=>Ok(()),
            result=self.poll_loop(bot,&options,&mut lease)=>result,
        };
        // FIFO submission fences any DB job whose caller was cancelled above.
        let released = self.release_source(lease).await;
        result?;
        released?;
        Ok(())
    }
    async fn poll_loop(
        &self,
        bot: &Bot,
        options: &PollOptions,
        lease: &mut SourceLease,
    ) -> Result<()> {
        loop {
            *lease = self.renew_source(lease).await?;
            let checkpoint = self.poll_checkpoint(lease).await?;
            // Recheck just before dispatch, including time spent awaiting the DB queue.
            if lease.expires_at_ms().saturating_sub(now()?) <= millis(options.request_budget)? {
                return Err(JournalError::new(JournalErrorKind::LeaseLost).into());
            }
            let params = GetUpdates::new()
                .limit(i64::from(options.batch_limit))
                .timeout(i64::from(options.poll_timeout))
                .allowed_updates(options.allowed_updates.clone());
            let params = if let Some(offset) = checkpoint.offset() {
                params.offset(offset)
            } else {
                params
            };
            let updates = bot
                .call(params)
                .retry_policy(
                    RetryPolicy::default()
                        .max_attempts(1)
                        .max_elapsed(options.request_budget),
                )
                .await?;
            if updates.len() > usize::from(options.batch_limit) {
                return Err(RuntimeError::Configuration);
            }
            let empty = updates.is_empty();
            self.append_poll(lease, &checkpoint, updates).await?;
            if empty {
                tokio::time::sleep(options.empty_poll_delay).await;
            }
        }
    }
    /// Replays unacknowledged work after failure. External side effects need stable operation IDs.
    /// Cancellation drains the current batch within shutdown_timeout, renewing its lease.
    pub async fn run_consumer_until<S: Send + Sync + 'static>(
        &self,
        bot: &Bot,
        consumer: impl Into<String>,
        mut router: Router<S>,
        options: ConsumerOptions,
        stop: CancellationToken,
    ) -> Result<()> {
        options.validate(self.lease_duration())?;
        let consumer = consumer.into();
        crate::journal::types::valid_id(&consumer)?;
        if stop.is_cancelled() {
            return Ok(());
        }
        let identity = tokio::select! { _=stop.cancelled()=>return Ok(()), result=bot.prepare_router(&mut router)=>result? };
        if i64::from(identity.id) != self.bot_id() {
            return Err(RuntimeError::Configuration);
        }
        let mut lease = self.claim_consumer(consumer).await?;
        let result = self
            .consume_loop(bot, Arc::new(router), &options, &stop, &mut lease)
            .await;
        let released = self.release_consumer(lease).await;
        result?;
        released?;
        Ok(())
    }
    async fn consume_loop<S: Send + Sync + 'static>(
        &self,
        bot: &Bot,
        router: Arc<Router<S>>,
        options: &ConsumerOptions,
        stop: &CancellationToken,
        lease: &mut ConsumerLease,
    ) -> Result<()> {
        while !stop.is_cancelled() {
            *lease = self.renew_consumer(lease).await?;
            let batch = tokio::select! { _=stop.cancelled()=>return Ok(()), result=self.read_consumer(lease,options.batch_limit)=>result? };
            if batch.events().is_empty() {
                tokio::select! { _=stop.cancelled()=>return Ok(()), _=tokio::time::sleep(options.idle_delay)=>{} }
                continue;
            }
            let (updates, ack) = batch.into_parts();
            // Reading may have waited for a write lock. Refresh before admitting handlers.
            *lease = self.renew_consumer(lease).await?;
            self.dispatch_consumer_batch(bot, router.clone(), updates, options, stop, lease)
                .await?;
            self.acknowledge_consumer(lease, ack).await?;
        }
        Ok(())
    }

    async fn dispatch_consumer_batch<S: Send + Sync + 'static>(
        &self,
        bot: &Bot,
        router: Arc<Router<S>>,
        updates: Vec<tg::Update>,
        options: &ConsumerOptions,
        stop: &CancellationToken,
        lease: &mut ConsumerLease,
    ) -> Result<()> {
        if remaining(lease)? < self.lease_duration().saturating_mul(2) / 3 {
            return Err(lease_lost());
        }
        let work = bot.dispatch_batch(router, updates, &options.dispatch);
        let keep_alive = self.keep_consumer_alive(lease);
        tokio::pin!(work, keep_alive);
        tokio::select! {
            result = &mut work => result.map_err(Into::into),
            result = &mut keep_alive => result,
            _ = stop.cancelled() => {
                tokio::time::timeout(options.shutdown_timeout, async {
                    tokio::select! {
                        result = &mut work => result.map_err(Into::into),
                        result = &mut keep_alive => result,
                    }
                }).await.map_err(|_| RuntimeError::ShutdownTimeout)?
            }
        }
    }

    async fn keep_consumer_alive(&self, lease: &mut ConsumerLease) -> Result<()> {
        loop {
            let remaining_before_sleep = remaining(lease)?;
            tokio::time::sleep((self.lease_duration() / 3).min(remaining_before_sleep / 3)).await;
            // A queued SQLite renewal is not evidence of renewed ownership. Abort handlers
            // before the known expiry if it cannot commit in time. The DB job may still run;
            // release waits behind it and fences its unchanged generation.
            let budget = remaining(lease)? / 2;
            *lease = tokio::time::timeout(budget, self.renew_consumer(lease))
                .await
                .map_err(|_| lease_lost())??;
        }
    }
}

fn remaining(lease: &ConsumerLease) -> Result<Duration> {
    let millis = lease.expires_at_ms().saturating_sub(now()?);
    if millis <= 0 {
        return Err(lease_lost());
    }
    Ok(Duration::from_millis(millis as u64))
}
fn lease_lost() -> RuntimeError {
    JournalError::new(JournalErrorKind::LeaseLost).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        journal::{Journal, JournalOptions},
        updates::UpdateOptions,
    };
    #[tokio::test]
    async fn a_blocked_heartbeat_cancels_handlers_before_the_known_lease_expires() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.sqlite");
        let journal = Journal::open(
            &path,
            JournalOptions::default().lease_duration(Duration::from_millis(180)),
        )
        .await
        .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
        feed.append_webhook(
            &source,
            serde_json::from_value(serde_json::json!({"update_id":1})).unwrap(),
        )
        .await
        .unwrap();
        let mut lease = feed.claim_consumer("router").await.unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let router = Arc::new(
            Router::with_state((started.clone(), dropped.clone())).on_update(|ctx| async move {
                let _flag = DropFlag(ctx.state().1.clone());
                ctx.state().0.notify_one();
                std::future::pending().await
            }),
        );
        let runner = {
            let feed = feed.clone();
            tokio::spawn(async move {
                feed.consume_loop(
                    &Bot::new("1:OFFLINE").unwrap(),
                    router,
                    &ConsumerOptions::default().idle_delay(Duration::from_millis(5)),
                    &CancellationToken::new(),
                    &mut lease,
                )
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        let mut blocker = rusqlite::Connection::open(&path).unwrap();
        let lock = blocker
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(240)).await;
        assert!(
            dropped.load(Ordering::SeqCst),
            "known lease expiry must cancel work even while its renewal is blocked in SQLite"
        );
        lock.commit().unwrap();
        assert!(
            matches!(runner.await.unwrap(),Err(RuntimeError::Journal(error)) if error.kind()==JournalErrorKind::LeaseLost)
        );
        journal.close().await.unwrap();
    }
}
