use super::types::{
    Action, ActionCall, ChatId, ExecutorError, ExecutorErrorKind, Request, Result, valid_id,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, RwLock, Weak},
    time::Duration,
};

/// Authenticated host identity. Deliberately not deserializable from tool input.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Principal(String);
impl Principal {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        valid_id(&value)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit all-chats authority must be chosen by the host.
#[derive(Clone)]
pub enum ChatScope {
    Only(BTreeSet<ChatId>),
    All,
}
impl ChatScope {
    pub fn only(values: impl IntoIterator<Item = ChatId>) -> Self {
        Self::Only(values.into_iter().collect())
    }
    pub fn permits(&self, chat: ChatId) -> bool {
        match self {
            Self::All => true,
            Self::Only(values) => values.contains(&chat),
        }
    }
}

/// Finite per-window limits; cost units are host estimates, not Telegram billing.
#[derive(Debug, Clone, Copy)]
pub struct BudgetLimits {
    pub invocations: u64,
    pub write_attempts: u64,
    pub upload_bytes: u64,
    pub cost_units: u64,
}
impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            invocations: 100,
            write_attempts: 20,
            upload_bytes: 50 * 1024 * 1024,
            cost_units: 1000,
        }
    }
}
/// Stable identity is retained across grant revisions and process restarts.
#[derive(Clone)]
pub struct Budget {
    id: String,
    window: Duration,
    limits: BudgetLimits,
}
impl Budget {
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        valid_id(&id)?;
        Ok(Self {
            id,
            window: Duration::from_secs(60),
            limits: BudgetLimits::default(),
        })
    }
    pub fn window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }
    pub fn limits(mut self, limits: BudgetLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn window_duration(&self) -> Duration {
        self.window
    }
    pub fn limit_values(&self) -> BudgetLimits {
        self.limits
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let limits = self.limits;
        if self.window < Duration::from_secs(1)
            || self.window > Duration::from_secs(365 * 24 * 3600)
            || !self.window.subsec_nanos().is_multiple_of(1_000_000)
            || [
                limits.invocations,
                limits.write_attempts,
                limits.upload_bytes,
                limits.cost_units,
            ]
            .iter()
            .any(|n| *n > i64::MAX as u64)
        {
            return Err(invalid_policy());
        }
        Ok(())
    }
}

/// One current host grant per authenticated principal and immutable bot profile.
/// Starts with no actions, chats, files or unscoped content authority.
#[derive(Clone)]
pub struct Grant {
    bot_id: i64,
    actions: BTreeSet<Action>,
    chats: ChatScope,
    files: BTreeSet<String>,
    unscoped_updates: bool,
    expires_at_ms: Option<i64>,
    approvals: BTreeSet<Action>,
    budget: Budget,
    costs: BTreeMap<Action, u64>,
}
impl Grant {
    pub fn new(bot_id: i64, budget: Budget) -> Self {
        Self {
            bot_id,
            actions: BTreeSet::new(),
            chats: ChatScope::only([]),
            files: BTreeSet::new(),
            unscoped_updates: false,
            expires_at_ms: None,
            approvals: BTreeSet::new(),
            budget,
            costs: BTreeMap::new(),
        }
    }
    pub fn allow(mut self, action: Action) -> Self {
        self.actions.insert(action);
        self
    }
    pub fn chats(mut self, scope: ChatScope) -> Self {
        self.chats = scope;
        self
    }
    pub fn files<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.files = values.into_iter().map(Into::into).collect();
        self
    }
    pub fn unscoped_updates(mut self, allowed: bool) -> Self {
        self.unscoped_updates = allowed;
        self
    }
    pub fn expires_at_ms(mut self, expiry: i64) -> Self {
        self.expires_at_ms = Some(expiry);
        self
    }
    pub fn require_approval(mut self, action: Action) -> Self {
        self.approvals.insert(action);
        self
    }
    pub fn action_cost(mut self, action: Action, units: u64) -> Self {
        self.costs.insert(action, units);
        self
    }
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn cost(&self, action: Action) -> u64 {
        self.costs.get(&action).copied().unwrap_or(1)
    }
    pub fn permits_chat(&self, chat: ChatId) -> bool {
        self.chats.permits(chat)
    }
    pub fn permits_file(&self, handle: &str) -> bool {
        self.files.contains(handle)
    }
    pub fn permits_unscoped_updates(&self) -> bool {
        self.unscoped_updates
    }
    fn validate(&self) -> Result<()> {
        self.budget.validate()?;
        if self.bot_id < 0
            || self.expires_at_ms.is_some_and(|v| v <= 0)
            || matches!(&self.chats, ChatScope::Only(values) if values.len() > 4096)
            || self.files.len() > 256
            || self.files.iter().any(|v| valid_id(v).is_err())
            || self.costs.values().any(|v| *v > i64::MAX as u64)
            || !self.approvals.is_subset(&self.actions)
        {
            return Err(invalid_policy());
        }
        Ok(())
    }
    fn check(&self, request: &Request, bot_id: i64, now_ms: i64) -> Result<()> {
        if now_ms < 0 {
            return Err(invalid_policy());
        }
        if self.bot_id != bot_id
            || self.expires_at_ms.is_some_and(|v| now_ms >= v)
            || !self.actions.contains(&request.action())
        {
            return Err(denied());
        }
        let (chat, file) = match request.call() {
            ActionCall::GetChat(args) => (Some(args.chat_id), None),
            ActionCall::SendMessage(args) => (Some(args.chat_id), None),
            ActionCall::EditMessageText(args) => (Some(args.chat_id), None),
            ActionCall::SendDocument(args) => (Some(args.chat_id), Some(args.file_handle.as_str())),
            ActionCall::AnswerCallbackQuery(_) | ActionCall::GetOperation(_) => {
                return Err(ExecutorError::new(ExecutorErrorKind::ContextRequired));
            }
            ActionCall::GetMe(_) | ActionCall::ReadUpdates(_) => (None, None),
        };
        if chat.is_some_and(|v| !self.permits_chat(v))
            || file.is_some_and(|v| !self.permits_file(v))
        {
            return Err(denied());
        }
        Ok(())
    }
}

struct Entry {
    revision: u64,
    grant: Grant,
}
#[derive(Default)]
struct State {
    revision: u64,
    grants: BTreeMap<(Principal, String), Entry>,
}
/// Shared in-process current policy. No lock is held across asynchronous work.
#[derive(Clone, Default)]
pub struct PolicyStore {
    inner: Arc<RwLock<State>>,
}
/// Host capability, not JSON data. Valid only for one store/revision/exact request.
#[derive(Clone)]
pub struct Approval {
    issuer: Weak<RwLock<State>>,
    principal: Principal,
    profile: String,
    fingerprint: [u8; 32],
    revision: u64,
    expires_at_ms: i64,
}
/// Local checks only. Neither a durable quota reservation nor an execution permit.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DryRun {
    pub schema_version: u32,
    pub action: Action,
    pub is_write: bool,
    pub grant_revision: u64,
}
impl PolicyStore {
    pub fn replace(&self, principal: &Principal, profile: &str, grant: Grant) -> Result<()> {
        valid_id(profile)?;
        grant.validate()?;
        let mut state = self.inner.write().map_err(|_| invalid_policy())?;
        let key = (principal.clone(), profile.to_owned());
        if state.grants.len() >= 1024 && !state.grants.contains_key(&key) {
            return Err(ExecutorError::new(ExecutorErrorKind::Capacity));
        }
        state.revision = state.revision.checked_add(1).ok_or_else(invalid_policy)?;
        let revision = state.revision;
        state.grants.insert(key, Entry { revision, grant });
        Ok(())
    }
    pub fn revoke(&self, principal: &Principal, profile: &str) -> Result<()> {
        valid_id(profile)?;
        self.inner
            .write()
            .map_err(|_| invalid_policy())?
            .grants
            .remove(&(principal.clone(), profile.to_owned()));
        Ok(())
    }
    /// Performs no identity RPC, journal/quota mutation or file access.
    /// References requiring journal/observation resolution fail closed for now.
    pub fn dry_run(
        &self,
        principal: &Principal,
        request: &Request,
        bot_id: i64,
        now_ms: i64,
        approval: Option<&Approval>,
    ) -> Result<DryRun> {
        self.authorized(principal, request, bot_id, now_ms, approval)
            .map(|(dry, _)| dry)
    }
    pub(crate) fn authorized(
        &self,
        principal: &Principal,
        request: &Request,
        bot_id: i64,
        now_ms: i64,
        approval: Option<&Approval>,
    ) -> Result<(DryRun, Grant)> {
        let state = self.inner.read().map_err(|_| invalid_policy())?;
        let entry = state
            .grants
            .get(&(principal.clone(), request.profile().to_owned()))
            .ok_or_else(denied)?;
        entry.grant.check(request, bot_id, now_ms)?;
        if entry.grant.approvals.contains(&request.action()) {
            let matches = approval.is_some_and(|value| {
                value
                    .issuer
                    .upgrade()
                    .is_some_and(|issuer| Arc::ptr_eq(&issuer, &self.inner))
                    && value.principal == *principal
                    && value.profile == request.profile()
                    && value.fingerprint == request.fingerprint()
                    && value.revision == entry.revision
                    && now_ms < value.expires_at_ms
            });
            if !matches {
                return Err(ExecutorError::new(ExecutorErrorKind::ApprovalRequired));
            }
        }
        Ok((
            DryRun {
                schema_version: 1,
                action: request.action(),
                is_write: request.action().is_write(),
                grant_revision: entry.revision,
            },
            entry.grant.clone(),
        ))
    }
    /// Explicit host confirmation. Approval includes operation_id, so the same
    /// intent under a fresh operation key needs fresh confirmation.
    pub fn approve(
        &self,
        principal: &Principal,
        request: &Request,
        bot_id: i64,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<Approval> {
        let state = self.inner.read().map_err(|_| invalid_policy())?;
        let entry = state
            .grants
            .get(&(principal.clone(), request.profile().to_owned()))
            .ok_or_else(denied)?;
        entry.grant.check(request, bot_id, now_ms)?;
        if expires_at_ms <= now_ms || entry.grant.expires_at_ms.is_some_and(|v| expires_at_ms > v) {
            return Err(invalid_policy());
        }
        Ok(Approval {
            issuer: Arc::downgrade(&self.inner),
            principal: principal.clone(),
            profile: request.profile().to_owned(),
            fingerprint: request.fingerprint(),
            revision: entry.revision,
            expires_at_ms,
        })
    }
}
fn invalid_policy() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
}
fn denied() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Denied)
}
macro_rules! redacted_debug {
    ($($name:ident),+ $(,)?) => { $(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name), "([REDACTED])")) }
        }
    )+ };
}
redacted_debug!(Principal, ChatScope, Budget, Grant, PolicyStore, Approval);
