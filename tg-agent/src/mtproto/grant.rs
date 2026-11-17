use super::{Action, Call, PeerId, Request, types::Result};
use crate::{
    executor::{Budget, ExecutorError, ExecutorErrorKind, valid_id},
    profiles::{ProfileIdentity, ProfileKind},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone)]
pub enum PeerScope {
    Only(BTreeSet<PeerId>),
    All,
}
impl PeerScope {
    pub fn only(values: impl IntoIterator<Item = PeerId>) -> Self {
        Self::Only(values.into_iter().collect())
    }
    pub fn permits(&self, peer: PeerId) -> bool {
        match self {
            Self::Only(v) => v.contains(&peer),
            Self::All => true,
        }
    }
}

/// Current host authority for one exact MTProto account. No authority by default.
#[derive(Clone)]
pub struct Grant {
    expected: ProfileIdentity,
    actions: BTreeSet<Action>,
    peers: PeerScope,
    files: BTreeSet<String>,
    unscoped_updates: bool,
    expiry: Option<i64>,
    approvals: BTreeSet<Action>,
    budget: Budget,
    costs: BTreeMap<Action, u64>,
}
impl Grant {
    pub fn new(expected: ProfileIdentity, budget: Budget) -> Result<Self> {
        if !matches!(
            expected.kind(),
            ProfileKind::MtprotoBot | ProfileKind::MtprotoUser
        ) {
            return Err(invalid_policy());
        }
        Ok(Self {
            expected,
            actions: BTreeSet::new(),
            peers: PeerScope::only([]),
            files: BTreeSet::new(),
            unscoped_updates: false,
            expiry: None,
            approvals: BTreeSet::new(),
            budget,
            costs: BTreeMap::new(),
        })
    }
    pub fn allow(mut self, action: Action) -> Self {
        self.actions.insert(action);
        self
    }
    pub fn peers(mut self, scope: PeerScope) -> Self {
        self.peers = scope;
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
    pub fn expires_at_ms(mut self, value: i64) -> Self {
        self.expiry = Some(value);
        self
    }
    pub fn require_approval(mut self, action: Action) -> Self {
        self.approvals.insert(action);
        self
    }
    pub fn action_cost(mut self, action: Action, cost: u64) -> Self {
        self.costs.insert(action, cost);
        self
    }
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn cost(&self, action: Action) -> u64 {
        self.costs.get(&action).copied().unwrap_or(1)
    }
    pub fn permits_peer(&self, peer: PeerId) -> bool {
        self.peers.permits(peer)
    }
    pub fn permits_file(&self, handle: &str) -> bool {
        self.files.contains(handle)
    }
    pub fn permits_unscoped_updates(&self) -> bool {
        self.unscoped_updates
    }
    pub(crate) fn expiry(&self) -> Option<i64> {
        self.expiry
    }
    pub(crate) fn requires_approval(&self, action: Action) -> bool {
        self.approvals.contains(&action)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.budget.validate()?;
        if !matches!(
            self.expected.kind(),
            ProfileKind::MtprotoBot | ProfileKind::MtprotoUser
        ) || (self.expected.kind() != ProfileKind::MtprotoBot
            && self.actions.contains(&Action::AnswerCallbackQuery))
            || self.expiry.is_some_and(|v| v <= 0)
            || matches!(&self.peers,PeerScope::Only(v) if v.len()>4096)
            || self.files.len() > 256
            || self.files.iter().any(|v| valid_id(v).is_err())
            || self.costs.values().any(|v| *v > i64::MAX as u64)
            || !self.approvals.is_subset(&self.actions)
        {
            return Err(invalid_policy());
        }
        Ok(())
    }
    pub(crate) fn check(
        &self,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
        scope: Option<&OriginalScope>,
    ) -> Result<()> {
        self.check_basic(request, expected, time)?;
        if let Some(scope) = scope {
            if !matches!(
                request.action(),
                Action::GetOperation | Action::AnswerCallbackQuery
            ) {
                return Err(invalid_policy());
            }
            self.check_original(request, expected, scope)?;
        } else {
            if matches!(
                request.call(),
                Call::GetOperation(_) | Call::AnswerCallbackQuery(_)
            ) {
                return Err(ExecutorError::new(ExecutorErrorKind::ContextRequired));
            }
            if request.call().peer().is_some_and(|p| !self.permits_peer(p))
                || request
                    .call()
                    .file_handle()
                    .is_some_and(|f| !self.permits_file(f))
            {
                return Err(denied());
            }
        }
        Ok(())
    }
    pub(crate) fn check_basic(
        &self,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
    ) -> Result<()> {
        if time < 0 {
            return Err(invalid_policy());
        }
        if self.expected != expected
            || self.expiry.is_some_and(|v| time >= v)
            || !self.actions.contains(&request.action())
        {
            return Err(denied());
        }
        Ok(())
    }
    fn check_original(
        &self,
        request: &Request,
        expected: ProfileIdentity,
        scope: &OriginalScope,
    ) -> Result<()> {
        if scope.account != expected
            || !scope.action.is_write()
            || !self.actions.contains(&scope.action)
            || (request.action() == Action::AnswerCallbackQuery
                && scope.action != Action::AnswerCallbackQuery)
            || (scope.action == Action::SendDocument) != scope.file_handle.is_some()
            || (scope.peer.is_none() && scope.action != Action::AnswerCallbackQuery)
            || scope
                .peer
                .map_or(!self.unscoped_updates, |p| !self.permits_peer(p))
            || scope
                .file_handle
                .as_ref()
                .is_some_and(|h| !self.permits_file(h))
        {
            return Err(denied());
        }
        Ok(())
    }
}
/// Policy context loaded by a trusted journal/reference provider, never tool JSON.
#[derive(Clone)]
pub(crate) struct OriginalScope {
    pub account: ProfileIdentity,
    pub action: Action,
    pub peer: Option<PeerId>,
    pub file_handle: Option<String>,
}
fn invalid_policy() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
}
fn denied() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Denied)
}
impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MtprotoGrant([REDACTED])")
    }
}
impl fmt::Debug for PeerScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerScope([REDACTED])")
    }
}
