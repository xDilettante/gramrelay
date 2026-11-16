use super::*;
use crate::updates::StoredUpdate;
use sha2::{Digest, Sha256};

/// Host clock trust, never a tool argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackClockPolicy {
    /// Requires monotonic ingestion provenance in this journal lifetime.
    JournalMonotonic,
    /// Allows historical/cross-process receipts; the host guarantees its UTC clock.
    TrustedWallClock,
}
#[derive(Clone)]
pub(super) struct ReferenceDeadline {
    pub(super) received_at_ms: i64,
    pub(super) expires_at_ms: i64,
    instant: Instant,
}
impl ReferenceDeadline {
    pub(super) fn cap_rpc(&self, deadline: Instant) -> Instant {
        deadline.min(self.instant)
    }
    pub(super) fn remaining(&self, time: i64) -> Result<Duration> {
        let wall = self
            .expires_at_ms
            .checked_sub(time)
            .filter(|v| *v > 0)
            .and_then(|v| u64::try_from(v).ok())
            .map(Duration::from_millis)
            .ok_or_else(expired)?;
        let mono = self
            .instant
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or_else(expired)?;
        Ok(wall.min(mono))
    }
}
/// Host-issued metadata. It is not deserializable and grants no extra authority.
#[derive(Clone)]
pub struct CallbackReference {
    query_id: String,
    scope: OperationScope,
    deadline: ReferenceDeadline,
}
impl CallbackReference {
    pub fn callback_query_id(&self) -> &str {
        &self.query_id
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.deadline.expires_at_ms
    }
    pub fn chat_id(&self) -> Option<ChatId> {
        self.scope.chat
    }
}
impl fmt::Debug for CallbackReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallbackReference([REDACTED])")
    }
}
type Key = (Principal, String, [u8; 32]);
#[derive(Default)]
pub(super) struct CallbackRegistry {
    entries: BTreeMap<Key, CallbackReference>,
    wall_high_water: i64,
}
impl CallbackRegistry {
    pub(super) fn prune(&mut self, time: i64) -> usize {
        self.wall_high_water = self.wall_high_water.max(time);
        let before = self.entries.len();
        self.entries
            .retain(|_, reference| reference.deadline.remaining(self.wall_high_water).is_ok());
        before - self.entries.len()
    }
    pub(super) fn register_event(
        &mut self,
        profile: &Profile,
        principal: &Principal,
        name: &str,
        event: &StoredUpdate,
        options: &ExecutorOptions,
        time: i64,
    ) -> Result<CallbackReference> {
        self.wall_high_water = self.wall_high_water.max(time);
        let reference = candidate(profile, event, options, self.wall_high_water)?;
        let key = (
            principal.clone(),
            name.to_owned(),
            callback_hash(&reference.query_id),
        );
        if let Some(existing) = self.entries.get(&key) {
            if existing.scope != reference.scope
                || existing.deadline.received_at_ms != reference.deadline.received_at_ms
                || existing.deadline.expires_at_ms != reference.deadline.expires_at_ms
            {
                return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
            }
            existing.deadline.remaining(self.wall_high_water)?;
            return Ok(existing.clone());
        }
        if self.entries.len() >= options.max_callback_references {
            return Err(ExecutorError::new(ExecutorErrorKind::Capacity));
        }
        self.entries.insert(key, reference.clone());
        Ok(reference)
    }
    fn lookup(
        &mut self,
        principal: &Principal,
        name: &str,
        id: &str,
        time: i64,
    ) -> Result<CallbackReference> {
        self.wall_high_water = self.wall_high_water.max(time);
        let reference = self
            .entries
            .get(&(principal.clone(), name.to_owned(), callback_hash(id)))
            .ok_or_else(unavailable)?;
        if reference.query_id != id {
            return Err(unavailable());
        }
        reference.deadline.remaining(self.wall_high_water)?;
        Ok(reference.clone())
    }
}
impl Executor {
    /// Explicit host observation from the same journal/profile. Never tool input.
    pub fn observe_callback(
        &self,
        principal: &Principal,
        profile: &str,
        event: &StoredUpdate,
    ) -> Result<CallbackReference> {
        valid_id(profile)?;
        let source = self
            .profiles
            .read()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .get(profile)
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Denied))?;
        let time = now()?;
        let reference = candidate(&source, event, &self.options, time)?;
        self.policy.check_callback_observation(
            principal,
            profile,
            source.bot_id,
            reference.scope.chat,
            time,
        )?;
        let mut registry = self
            .callback_references
            .lock()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?;
        registry.prune(time);
        registry.register_event(&source, principal, profile, event, &self.options, time)
    }
    pub fn prune_callback_references(&self) -> Result<usize> {
        Ok(self
            .callback_references
            .lock()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .prune(now()?))
    }
    pub(super) fn callback_context(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
    ) -> Result<(OperationScope, ReferenceDeadline)> {
        self.policy
            .check_lookup(principal, request, profile.bot_id, now()?)?;
        let ActionCall::AnswerCallbackQuery(args) = request.call() else {
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidInput));
        };
        let reference = self
            .callback_references
            .lock()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .lookup(
                principal,
                request.profile(),
                &args.callback_query_id,
                now()?,
            )?;
        Ok((reference.scope, reference.deadline))
    }
}
fn candidate(
    profile: &Profile,
    event: &StoredUpdate,
    options: &ExecutorOptions,
    time: i64,
) -> Result<CallbackReference> {
    if !event.is_from(&profile.journal)
        || !event
            .cursor()
            .matches_profile(profile.feed.executor_binding().1)
    {
        return Err(ExecutorError::new(ExecutorErrorKind::Denied));
    }
    let update = event.update();
    if update.known_event_count() != 1
        || !update.unknown_fields().is_empty()
        || read::ambiguous_callback(update)
    {
        return Err(ExecutorError::new(ExecutorErrorKind::InvalidInput));
    }
    let callback = update
        .callback_query
        .as_ref()
        .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::InvalidInput))?;
    let id = callback.id.as_str();
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(ExecutorError::new(ExecutorErrorKind::InvalidInput));
    }
    let received = event.received_at_ms();
    if received < 0 || received > time {
        return Err(expired());
    }
    let expires = received
        .checked_add(options.callback_ttl.as_millis() as i64)
        .ok_or_else(expired)?;
    let remaining = expires
        .checked_sub(time)
        .filter(|v| *v > 0)
        .and_then(|v| u64::try_from(v).ok())
        .map(Duration::from_millis)
        .ok_or_else(expired)?;
    let wall_deadline = Instant::now().checked_add(remaining).ok_or_else(expired)?;
    let instant = match (options.callback_clock_policy, event.callback_received()) {
        (_, Some(source)) => Instant::from_std(source)
            .checked_add(options.callback_ttl)
            .ok_or_else(expired)?
            .min(wall_deadline),
        (CallbackClockPolicy::TrustedWallClock, None) => wall_deadline,
        (CallbackClockPolicy::JournalMonotonic, None) => return Err(unavailable()),
    };
    let deadline = ReferenceDeadline {
        received_at_ms: received,
        expires_at_ms: expires,
        instant,
    };
    deadline.remaining(time)?;
    let chat = callback
        .message
        .as_ref()
        .map(|message| ChatId::new(i64::from(message.chat().id)))
        .transpose()
        .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidInput))?;
    let scope = OperationScope {
        bot_id: profile.bot_id,
        action: Action::AnswerCallbackQuery,
        chat,
        file: None,
        reference_hash: Some(callback_hash(id)),
    };
    Ok(CallbackReference {
        query_id: id.to_owned(),
        scope,
        deadline,
    })
}
fn callback_hash(id: &str) -> [u8; 32] {
    // Stable across owners, registries, timestamps and process restarts. The
    // database's profile + hash unique index fences all admitted aliases.
    let mut hash = Sha256::new();
    hash.update(b"tg-agent/callback/v1\0");
    hash.update(id.as_bytes());
    hash.finalize().into()
}
fn expired() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::ReferenceExpired)
}
fn unavailable() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::ReferenceUnavailable)
}
