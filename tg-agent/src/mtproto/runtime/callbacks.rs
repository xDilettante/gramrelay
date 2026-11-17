#[cfg(all(test, feature = "mtproto-test-kit"))]
#[path = "callback_tests.rs"]
mod tests;
use super::*;
pub use crate::executor::CallbackClockPolicy;
use crate::executor::callback_clock::CallbackClock;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub(super) struct Deadline {
    pub received_at_ms: i64,
    pub expires_at_ms: i64,
    instant: Instant,
    clock: CallbackClock,
}
impl Deadline {
    pub(super) fn remaining(&self, time: i64) -> Result<Duration> {
        let time = self.clock.observe(time);
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
            .filter(|d| !d.is_zero())
            .ok_or_else(expired)?;
        Ok(wall.min(mono))
    }
    pub(super) fn cap(&self, deadline: Instant) -> Instant {
        deadline.min(self.instant)
    }
}
#[derive(Clone)]
pub(super) struct Context {
    pub query: i64,
    pub scope: Scope,
    pub deadline: Deadline,
}
impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoCallbackContext([REDACTED])")
    }
}
/// A host-observed opaque alias. No Serialize/Deserialize or extra permissions.
#[derive(Clone)]
pub struct CallbackReference {
    id: String,
    pub(super) context: Context,
}
impl CallbackReference {
    pub fn reference(&self) -> &str {
        &self.id
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.context.deadline.expires_at_ms
    }
    pub fn peer(&self) -> Option<PeerId> {
        self.context.scope.peer
    }
}
impl std::fmt::Debug for CallbackReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoCallbackReference([REDACTED])")
    }
}
type Key = (Principal, String, [u8; 32]);
#[derive(Default)]
pub(super) struct Registry {
    entries: BTreeMap<Key, CallbackReference>,
    ids: BTreeMap<String, Key>,
    clock: CallbackClock,
}
impl Registry {
    pub(super) fn prune(&mut self, time: i64) -> usize {
        let time = self.clock.observe(time);
        let before = self.entries.len();
        self.entries
            .retain(|_, r| r.context.deadline.remaining(time).is_ok());
        self.ids.retain(|_, key| self.entries.contains_key(key));
        before - self.entries.len()
    }
    pub(super) fn register(
        &mut self,
        principal: &Principal,
        profile: &str,
        mut context: Context,
        limit: usize,
        time: i64,
    ) -> Result<CallbackReference> {
        let time = self
            .clock
            .observe(time.max(context.deadline.clock.highest()));
        context.deadline.clock = self.clock.clone();
        context.deadline.remaining(time)?;
        let key = (
            principal.clone(),
            profile.to_owned(),
            context.scope.reference_hash.ok_or_else(unavailable)?,
        );
        if let Some(old) = self.entries.get(&key) {
            if old.context.query != context.query
                || old.context.scope != context.scope
                || old.context.deadline.received_at_ms != context.deadline.received_at_ms
                || old.context.deadline.expires_at_ms != context.deadline.expires_at_ms
            {
                return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
            }
            old.context.deadline.remaining(time)?;
            return Ok(old.clone());
        }
        if self.entries.len() >= limit {
            self.prune(time);
            if self.entries.len() >= limit {
                return Err(capacity());
            }
        }
        for _ in 0..4 {
            use std::fmt::Write;
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
            let mut id = String::with_capacity(37);
            id.push_str("mtcb_");
            for b in bytes {
                write!(&mut id, "{b:02x}").expect("String formatting");
            }
            if self.ids.contains_key(&id) {
                continue;
            }
            let reference = CallbackReference {
                id: id.clone(),
                context,
            };
            self.ids.insert(id, key.clone());
            self.entries.insert(key, reference.clone());
            return Ok(reference);
        }
        Err(capacity())
    }
    pub(super) fn lookup(
        &mut self,
        principal: &Principal,
        profile: &str,
        id: &str,
        time: i64,
    ) -> Result<CallbackReference> {
        let time = self.clock.observe(time);
        let key = self
            .ids
            .get(id)
            .filter(|k| &k.0 == principal && k.1 == profile)
            .ok_or_else(unavailable)?;
        let reference = self.entries.get(key).ok_or_else(unavailable)?;
        reference.context.deadline.remaining(time)?;
        Ok(reference.clone())
    }
}
fn candidate(
    profile: &Profile,
    observation: &updates::CallbackObservation,
    options: &ExecutorOptions,
    time: i64,
) -> Result<Context> {
    if profile.expected.kind() != ProfileKind::MtprotoBot
        || !Arc::ptr_eq(&observation.origin, &profile.journal.origin_token())
        || !observation.cursor.matches_profile(&profile.name)
    {
        return Err(denied());
    }
    let received = observation.received_at_ms;
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
    let instant = match (options.callback_clock_policy, observation.observed) {
        (_, Some(source)) => source
            .checked_add(options.callback_ttl)
            .ok_or_else(expired)?
            .min(wall_deadline),
        (CallbackClockPolicy::TrustedWallClock, None) => wall_deadline,
        (CallbackClockPolicy::JournalMonotonic, None) => return Err(unavailable()),
    };
    let deadline = Deadline {
        received_at_ms: received,
        expires_at_ms: expires,
        instant,
        clock: CallbackClock::default(),
    };
    deadline.remaining(time)?;
    Ok(Context {
        query: observation.query,
        scope: Scope {
            account: profile.expected,
            action: Action::AnswerCallbackQuery,
            peer: observation.peer,
            file: None,
            reference_hash: Some(callback_hash(observation.query)),
        },
        deadline,
    })
}
fn callback_hash(query: i64) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"tg-agent/mtproto/callback/v1\0");
    hash.update(query.to_le_bytes());
    hash.finalize().into()
}
pub(super) fn payload(request: &Request, context: &Context) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(request).map_err(|_| invalid_policy())?;
    value["call"]["arguments"]["reference"] =
        serde_json::json!(context.scope.reference_hash.ok_or_else(unavailable)?);
    Ok(
        serde_json::json!({"request":value,"reference":{"received_at_ms":context.deadline.received_at_ms,"expires_at_ms":context.deadline.expires_at_ms}}),
    )
}
impl Executor {
    /// Explicit same-journal host observation; tool JSON cannot supply this proof.
    pub fn observe_callback(
        &self,
        principal: &Principal,
        profile: &str,
        event: &updates::StoredEvent,
    ) -> Result<CallbackReference> {
        crate::executor::valid_id(profile)?;
        let jobs = self.jobs.try_lock().map_err(|_| capacity())?;
        if jobs.closed {
            return Err(closed());
        }
        let source = self
            .profiles
            .read()
            .map_err(|_| invalid_policy())?
            .get(profile)
            .cloned()
            .ok_or_else(denied)?;
        let observation = event
            .callback_observation()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::InvalidInput))?;
        let time = now()?;
        let context = candidate(&source, &observation, &self.options, time)?;
        self.policy.check_mtproto_callback_observation(
            principal,
            profile,
            source.expected,
            context.scope.peer,
            time,
        )?;
        let mut registry = self
            .callback_references
            .lock()
            .map_err(|_| invalid_policy())?;
        let time = now()?;
        registry.prune(time);
        registry.register(
            principal,
            profile,
            context,
            self.options.max_callback_references,
            time,
        )
    }
    pub fn prune_callback_references(&self) -> Result<usize> {
        Ok(self
            .callback_references
            .lock()
            .map_err(|_| invalid_policy())?
            .prune(now()?))
    }
}
impl Worker {
    pub(super) fn attach_callbacks(
        &self,
        principal: &Principal,
        page: &mut updates::ReadPage,
    ) -> Result<()> {
        let mut registry = self
            .callback_references
            .lock()
            .map_err(|_| invalid_policy())?;
        let time = now()?;
        registry.prune(time);
        page.attach_callbacks(|observation| {
            let context = candidate(&self.profile, observation, &self.options, time).ok()?;
            self.policy
                .check_mtproto_callback_observation(
                    principal,
                    &self.profile.name,
                    self.profile.expected,
                    context.scope.peer,
                    time,
                )
                .ok()?;
            registry
                .register(
                    principal,
                    &self.profile.name,
                    context,
                    self.options.max_callback_references,
                    now().ok()?,
                )
                .ok()
                .map(|r| r.id)
        });
        Ok(())
    }
}
fn expired() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::ReferenceExpired)
}
fn unavailable() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::ReferenceUnavailable)
}
