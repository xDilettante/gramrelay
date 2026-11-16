use crate::{Bot, ChatId, EffectOutcome, Error, ErrorKind, Result, Router, Update, User};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

/// Bounded batch dispatch. Ordering is guaranteed within one awaited batch.
/// Callers must serialize batches for the same feed and own acknowledgement/recovery.
#[derive(Debug, Clone)]
pub struct DispatchOptions {
    max_concurrency: usize,
    handler_timeout: Duration,
}
impl Default for DispatchOptions {
    fn default() -> Self {
        Self {
            max_concurrency: 16,
            handler_timeout: Duration::from_secs(30),
        }
    }
}
impl DispatchOptions {
    pub fn max_concurrency(mut self, limit: usize) -> Self {
        self.max_concurrency = limit;
        self
    }
    pub fn handler_timeout(mut self, timeout: Duration) -> Self {
        self.handler_timeout = timeout;
        self
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.max_concurrency) {
            return Err(Error::local(ErrorKind::Validation, "max_concurrency"));
        }
        if self.handler_timeout.is_zero()
            || Instant::now().checked_add(self.handler_timeout).is_none()
        {
            return Err(Error::local(ErrorKind::Validation, "handler_timeout"));
        }
        Ok(())
    }
}
impl Bot {
    /// Validate routes and bind command mentions to a verified getMe identity.
    pub async fn prepare_router<S: Send + Sync + 'static>(
        &self,
        router: &mut Router<S>,
    ) -> Result<User> {
        router.validate()?;
        let identity = self.verified_identity().await?;
        router.bind_username(identity.username.as_deref())?;
        Ok(identity)
    }
    /// Dispatch at most 100 events with per-chat admission order and an unknown-scope barrier.
    /// No getUpdates call or acknowledgement is performed. Dropping this future aborts handlers;
    /// effects already started by a handler may have an unknown outcome.
    pub async fn dispatch_batch<S: Send + Sync + 'static>(
        &self,
        router: Arc<Router<S>>,
        updates: Vec<Update>,
        options: &DispatchOptions,
    ) -> Result<()> {
        options.validate()?;
        router.validate()?;
        if updates.len() > 100
            || updates
                .iter()
                .any(|u| u.update_id < 0 || u.known_event_count() > 1)
        {
            return Err(Error::local(ErrorKind::Validation, "update_batch"));
        }
        process_batch(self, router, updates, options).await
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Chat(ChatId),
    Global,
}

fn key(update: &Update) -> Key {
    update.chat_id().map_or(Key::Global, Key::Chat)
}

async fn process_batch<S: Send + Sync + 'static>(
    bot: &Bot,
    router: Arc<Router<S>>,
    updates: Vec<Update>,
    options: &DispatchOptions,
) -> Result<()> {
    // Unknown scopes must not race a possibly related chat. This includes inline callbacks.
    let unscoped = updates.iter().any(|update| key(update) == Key::Global);
    let mut groups: Vec<(Key, Vec<Update>)> = Vec::new();
    for update in updates {
        let event_key = if unscoped { Key::Global } else { key(&update) };
        // ponytail: linear lookup is bounded by Telegram's 100-event batch.
        if let Some((_, events)) = groups
            .iter_mut()
            .find(|(existing, _)| *existing == event_key)
        {
            events.push(update);
        } else {
            groups.push((event_key, vec![update]));
        }
    }
    let mut pending: VecDeque<_> = groups.into_iter().map(|(_, events)| events).collect();
    let mut running = JoinSet::new();
    for _ in 0..options.max_concurrency {
        if let Some(events) = pending.pop_front() {
            spawn_group(
                &mut running,
                bot.clone(),
                router.clone(),
                events,
                options.handler_timeout,
            );
        }
    }
    let mut failure = None;
    while let Some(joined) = running.join_next().await {
        let result = joined.unwrap_or_else(|_| Err(Error::handler("handler_panic")));
        if let Err(error) = result
            && failure.is_none()
        {
            failure = Some(error);
        }
        if failure.is_none()
            && let Some(events) = pending.pop_front()
        {
            spawn_group(
                &mut running,
                bot.clone(),
                router.clone(),
                events,
                options.handler_timeout,
            );
        }
    }
    failure.map_or(Ok(()), Err)
}

fn spawn_group<S: Send + Sync + 'static>(
    running: &mut JoinSet<Result<()>>,
    bot: Bot,
    router: Arc<Router<S>>,
    events: Vec<Update>,
    timeout: Duration,
) {
    running.spawn(async move {
        for update in events {
            tokio::time::timeout(timeout, router.dispatch(&bot, update))
                .await
                .map_err(|_| Error::new(ErrorKind::Timeout, EffectOutcome::Unknown))??;
        }
        Ok(())
    });
}
