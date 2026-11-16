use crate::{
    Bot, EffectOutcome, Error, ErrorKind, Result,
    api::{Effect, Method, ReplaySafety},
};
use std::time::{Duration, Instant};

/// Explicit bounded retries for generated JSON requests. No policy is enabled by default.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    attempts: u8,
    elapsed: Duration,
    initial: Duration,
    delay_cap: Duration,
    rejected_writes: bool,
    jitter: bool,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            elapsed: Duration::from_secs(30),
            initial: Duration::from_millis(100),
            delay_cap: Duration::from_secs(5),
            rejected_writes: false,
            jitter: true,
        }
    }
}
impl RetryPolicy {
    pub fn jitter(mut self, enabled: bool) -> Self {
        self.jitter = enabled;
        self
    }
    pub fn max_attempts(mut self, attempts: u8) -> Self {
        self.attempts = attempts;
        self
    }
    pub fn max_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial = delay;
        self
    }
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.delay_cap = delay;
        self
    }
    /// Allow confirmed Bot API 429 refusals for ordinary JSON writes/acknowledgements.
    /// Unknown, credential/session and short-lived-reference operations remain ineligible.
    pub fn retry_rejected_writes(mut self, enabled: bool) -> Self {
        self.rejected_writes = enabled;
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=10).contains(&self.attempts)
            || self.elapsed.is_zero()
            || Instant::now().checked_add(self.elapsed).is_none()
            || self.initial > self.delay_cap
        {
            return Err(Error::local(ErrorKind::Validation, "retry_policy"));
        }
        Ok(())
    }
    pub(crate) fn stream_timeout(&self) -> Result<Duration> {
        self.validate()?;
        if self.rejected_writes && self.attempts > 1 {
            return Err(Error::local(ErrorKind::Validation, "retry_attachments"));
        }
        Ok(self.elapsed)
    }
}

pub(crate) async fn run_json<M: Method>(
    bot: &Bot,
    params: &M,
    policy: Option<RetryPolicy>,
) -> Result<M::Response> {
    if let Some(policy) = &policy {
        policy.validate()?;
    }
    let deadline = policy
        .as_ref()
        .map(|policy| {
            Instant::now()
                .checked_add(policy.elapsed)
                .ok_or_else(|| Error::local(ErrorKind::Validation, "retry_policy"))
        })
        .transpose()?;
    let max_attempts = policy.as_ref().map_or(1, |policy| policy.attempts);
    let mut previous = None;
    for attempt in 1..=max_attempts {
        let remaining = if let Some(end) = deadline {
            match end
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
            {
                Some(remaining) => Some(remaining),
                None => {
                    return Err(previous
                        .unwrap_or_else(|| Error::local(ErrorKind::Timeout, "retry_policy")));
                }
            }
        } else {
            None
        };
        let result = bot
            .perform_with_timeout(
                async {
                    if !params.attachment_references().is_empty() {
                        return Err(Error::local(ErrorKind::Validation, "attachments"));
                    }
                    Ok(bot.post(M::NAME)?.json(params))
                },
                remaining,
            )
            .await;
        let error = match result {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if attempt == max_attempts {
            return Err(error);
        }
        let policy = policy
            .as_ref()
            .expect("multiple attempts require an explicit policy");
        if !eligible::<M>(&error, params, policy) {
            return Err(error);
        }
        let base = policy
            .initial
            .checked_mul(1u32 << (attempt - 1))
            .unwrap_or(policy.delay_cap)
            .min(policy.delay_cap);
        // Non-cryptographic full jitter, never reducing the server's minimum delay.
        let backoff = if policy.jitter {
            Duration::try_from_secs_f64(base.as_secs_f64() * fastrand::f64())
                .unwrap_or(base)
                .min(base)
        } else {
            base
        };
        let delay = backoff.max(error.retry_after().unwrap_or_default());
        if delay > policy.delay_cap
            || deadline.is_some_and(|end| {
                Instant::now()
                    .checked_add(delay)
                    .is_none_or(|wake| wake >= end)
            })
        {
            return Err(error);
        }
        previous = Some(error);
        // The completed exchange released its permit. Cancellation leaves no detached retry.
        tokio::time::sleep(delay).await;
    }
    unreachable!("a validated policy always permits at least one attempt")
}

fn eligible<M: Method>(error: &Error, params: &M, policy: &RetryPolicy) -> bool {
    if error.outcome() == EffectOutcome::NotSent || error.migrate_to_chat_id().is_some() {
        return false;
    }
    let read = M::REPLAY == ReplaySafety::ReadOnly;
    if error.outcome() == EffectOutcome::Rejected && error.api_error_code() == Some(429) {
        return read
            || policy.rejected_writes
                && matches!(M::EFFECT, Effect::Write | Effect::Acknowledge)
                && serde_json::to_value(params).is_ok_and(|value| !has_temporal_reference(&value));
    }
    read && (matches!(error.kind(), ErrorKind::Transport | ErrorKind::Timeout)
        || error.kind() == ErrorKind::Http
            && error
                .http_status()
                .is_some_and(|status| (500..600).contains(&status)))
}

fn has_temporal_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            (!value.is_null() && (key.starts_with("ephemeral_") || key.ends_with("_query_id")))
                || has_temporal_reference(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(has_temporal_reference),
        _ => false,
    }
}
