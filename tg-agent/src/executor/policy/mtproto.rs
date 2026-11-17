use super::*;
use crate::{
    mtproto::{Action, Grant, OriginalScope, Request},
    profiles::ProfileIdentity,
};

/// A local MTProto policy result, not a quota reservation or an execution permit.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MtprotoDryRun {
    pub schema_version: u32,
    pub action: Action,
    pub is_write: bool,
    pub grant_revision: u64,
}

impl Entry {
    fn mtproto(&self) -> Result<&Grant> {
        match &self.grant {
            GrantKind::Mtproto(grant) => Ok(grant),
            GrantKind::Bot(_) => Err(denied()),
        }
    }
}
impl PolicyStore {
    pub(crate) fn check_mtproto_lookup(
        &self,
        principal: &Principal,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
    ) -> Result<()> {
        if request.action() != Action::GetOperation {
            return Err(invalid_policy());
        }
        let state = self.inner.read().map_err(|_| invalid_policy())?;
        let entry = state
            .grants
            .get(&(principal.clone(), request.profile().to_owned()))
            .ok_or_else(denied)?;
        entry.mtproto()?.check_basic(request, expected, time)
    }
    pub fn replace_mtproto(
        &self,
        principal: &Principal,
        profile: &str,
        grant: Grant,
    ) -> Result<()> {
        valid_id(profile)?;
        grant.validate()?;
        self.store(principal, profile, GrantKind::Mtproto(grant))
    }
    pub fn dry_run_mtproto(
        &self,
        principal: &Principal,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
        approval: Option<&Approval>,
    ) -> Result<MtprotoDryRun> {
        self.authorized_mtproto(principal, request, expected, time, approval, None)
            .map(|(dry, _)| dry)
    }
    pub(crate) fn authorized_mtproto(
        &self,
        principal: &Principal,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
        approval: Option<&Approval>,
        scope: Option<&OriginalScope>,
    ) -> Result<(MtprotoDryRun, Grant)> {
        let state = self.inner.read().map_err(|_| invalid_policy())?;
        let entry = state
            .grants
            .get(&(principal.clone(), request.profile().to_owned()))
            .ok_or_else(denied)?;
        let grant = entry.mtproto()?;
        grant.check(request, expected, time, scope)?;
        if grant.requires_approval(request.action())
            && !approval.is_some_and(|a| {
                a.matches(
                    self,
                    principal,
                    request.profile(),
                    request.fingerprint(),
                    entry.revision,
                    time,
                )
            })
        {
            return Err(ExecutorError::new(ExecutorErrorKind::ApprovalRequired));
        }
        Ok((
            MtprotoDryRun {
                schema_version: 1,
                action: request.action(),
                is_write: request.action().is_write(),
                grant_revision: entry.revision,
            },
            grant.clone(),
        ))
    }
    pub fn approve_mtproto(
        &self,
        principal: &Principal,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
        expires_at_ms: i64,
    ) -> Result<Approval> {
        self.approve_mtproto_with_scope(principal, request, expected, time, expires_at_ms, None)
    }
    pub(crate) fn approve_mtproto_with_scope(
        &self,
        principal: &Principal,
        request: &Request,
        expected: ProfileIdentity,
        time: i64,
        expires_at_ms: i64,
        scope: Option<&OriginalScope>,
    ) -> Result<Approval> {
        let state = self.inner.read().map_err(|_| invalid_policy())?;
        let entry = state
            .grants
            .get(&(principal.clone(), request.profile().to_owned()))
            .ok_or_else(denied)?;
        let grant = entry.mtproto()?;
        grant.check(request, expected, time, scope)?;
        if expires_at_ms <= time || grant.expiry().is_some_and(|expiry| expires_at_ms > expiry) {
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

#[cfg(test)]
#[path = "mtproto_tests.rs"]
mod tests;
