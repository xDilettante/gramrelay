//! Managed HTTP intake committed to the update journal. Processing is a separate consumer.
use crate::{
    journal::{JournalError, JournalErrorKind, now},
    runtime::{Result, RuntimeError},
    updates::{SourceLease, SourceMode, UpdateFeed},
};
use std::{fmt, time::Duration};
use tg::{
    Bot, CancellationToken, Update,
    webhook::{
        WebhookError, WebhookFuture, WebhookIngress, WebhookOptions, WebhookSecret, WebhookSink,
    },
};
use tokio::{net::TcpListener, sync::watch, time::Instant};

pub struct DurableWebhook {
    feed: UpdateFeed,
    bot: Bot,
    secret: WebhookSecret,
    options: WebhookOptions,
}
impl fmt::Debug for DurableWebhook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableWebhook").finish_non_exhaustive()
    }
}
impl UpdateFeed {
    /// Construct without network/ownership side effects. Startup verifies this feed's bot ID.
    pub fn webhook(&self, bot: &Bot, secret: WebhookSecret) -> DurableWebhook {
        DurableWebhook {
            feed: self.clone(),
            bot: bot.clone(),
            secret,
            options: WebhookOptions::default(),
        }
    }
}
impl DurableWebhook {
    pub fn options(mut self, options: WebhookOptions) -> Self {
        self.options = options;
        self
    }
    /// Cancel the token and await cleanup before closing the journal or replacing the source.
    pub async fn serve_until(self, listener: TcpListener, stop: CancellationToken) -> Result<()> {
        let Self {
            feed,
            bot,
            secret,
            options,
        } = self;
        options.validate_listener(&listener)?;
        if options.request_budget() > feed.lease_duration() / 2
            || feed.lease_duration() / 3 < Duration::from_millis(1)
        {
            return Err(RuntimeError::Configuration);
        }
        if stop.is_cancelled() {
            return Ok(());
        }
        let _guard = bot.claim_update_source()?;
        let identity = tokio::select! {
            _=stop.cancelled()=>return Ok(()),
            result=bot.verified_identity()=>result?,
        };
        if i64::from(identity.id) != feed.bot_id() {
            return Err(RuntimeError::Configuration);
        }
        // Do not cancel an admitted claim: first observe its result, then release if stopping.
        let mut lease = feed.claim_source(SourceMode::Webhook).await?;
        let (lease_updates, leases) = watch::channel(lease.clone());
        let (failure, mut failures) = watch::channel(None);
        let sink = DurableSink {
            feed: feed.clone(),
            leases,
            failure,
        };
        let ingress = match WebhookIngress::new(secret, options, sink) {
            Ok(ingress) => ingress,
            Err(error) => {
                feed.release_source(lease).await?;
                return Err(error.into());
            }
        };
        let serving_stop = stop.child_token();
        let result = {
            let heartbeat = heartbeat(&feed, &mut lease, &lease_updates);
            tokio::pin!(heartbeat);
            let serving = ingress.serve_until(listener, serving_stop.clone());
            tokio::pin!(serving);
            tokio::select! {
                result=&mut serving=>result.map_err(Into::into),
                result=&mut heartbeat=>{
                    ingress.abort();serving_stop.cancel();let _=serving.await;result
                }
                error=source_failure(&mut failures)=>{
                    ingress.abort();serving_stop.cancel();let _=serving.await;Err(RuntimeError::Journal(error))
                }
            }
        };
        // Also drains a renewal/append whose async caller timed out after native admission.
        let released = feed.release_source(lease).await;
        result?;
        released?;
        Ok(())
    }
}
struct DurableSink {
    feed: UpdateFeed,
    leases: watch::Receiver<SourceLease>,
    failure: watch::Sender<Option<JournalErrorKind>>,
}
impl WebhookSink for DurableSink {
    fn ingest(&self, update: Update, deadline: Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            if self.failure.borrow().is_some() {
                return Err(WebhookError::Unavailable);
            }
            let lease = self.leases.borrow().clone();
            let result =
                tokio::time::timeout_at(deadline, self.feed.append_webhook(&lease, update))
                    .await
                    .map_err(|_| WebhookError::Timeout)?;
            match result {
                Ok(_) => Ok(()),
                Err(error) => {
                    if matches!(
                        error.kind(),
                        JournalErrorKind::LeaseLost | JournalErrorKind::Closed
                    ) {
                        self.failure.send_replace(Some(error.kind()));
                    }
                    Err(match error.kind() {
                        JournalErrorKind::Conflict => WebhookError::Conflict,
                        JournalErrorKind::Capacity => WebhookError::Busy,
                        JournalErrorKind::Validation => WebhookError::BadRequest,
                        _ => WebhookError::Unavailable,
                    })
                }
            }
        })
    }
}
async fn heartbeat(
    feed: &UpdateFeed,
    lease: &mut SourceLease,
    updates: &watch::Sender<SourceLease>,
) -> Result<()> {
    loop {
        let left = remaining(lease)?;
        tokio::time::sleep((feed.lease_duration() / 3).min(left / 3)).await;
        let budget = remaining(lease)? / 2;
        *lease = tokio::time::timeout(budget, feed.renew_source_fenced(lease))
            .await
            .map_err(|_| lost())??;
        updates.send_replace(lease.clone());
    }
}
fn remaining(lease: &SourceLease) -> Result<Duration> {
    let left = lease.expires_at_ms().saturating_sub(now()?);
    if left <= 0 {
        return Err(lost());
    }
    Ok(Duration::from_millis(left as u64))
}
fn lost() -> RuntimeError {
    JournalError::new(JournalErrorKind::LeaseLost).into()
}
async fn source_failure(failures: &mut watch::Receiver<Option<JournalErrorKind>>) -> JournalError {
    loop {
        if let Some(kind) = *failures.borrow_and_update() {
            return JournalError::new(kind);
        }
        if failures.changed().await.is_err() {
            return JournalError::new(JournalErrorKind::Closed);
        }
    }
}
