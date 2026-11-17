use super::*;
use crate::{
    executor::{ExecutorError, ExecutorErrorKind, PolicyTime, file_store, storage_error},
    journal::{
        self, Completion, Journal, JournalError, JournalErrorKind, OperationRequest, Preparation,
        authorization::{BudgetContext, charge_invocation_checked},
        mtproto_authorization::Scope,
    },
    profiles::{ProfileIdentity, ProfileKind},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tg::EffectOutcome;
use tokio::{
    sync::{Mutex, Semaphore, oneshot},
    task::JoinSet,
};
mod callbacks;
mod files;
mod output;
mod write;
pub use crate::executor::{FileLimits, RegisteredFile};
pub use callbacks::{CallbackClockPolicy, CallbackReference};
pub use output::*;
type Result<T> = std::result::Result<T, ExecutorError>;

#[derive(Debug, Clone)]
pub struct ExecutorOptions {
    max_in_flight: usize,
    max_read_bytes: usize,
    receipt_margin: Duration,
    callback_ttl: Duration,
    max_callback_references: usize,
    callback_clock_policy: CallbackClockPolicy,
    file_limits: FileLimits,
}
impl Default for ExecutorOptions {
    fn default() -> Self {
        Self {
            max_in_flight: 16,
            max_read_bytes: 1024 * 1024,
            receipt_margin: Duration::from_secs(5),
            callback_ttl: Duration::from_secs(10),
            max_callback_references: 1024,
            callback_clock_policy: CallbackClockPolicy::JournalMonotonic,
            file_limits: FileLimits::default(),
        }
    }
}
impl ExecutorOptions {
    pub fn file_limits(mut self, value: FileLimits) -> Self {
        self.file_limits = value;
        self
    }
    pub fn callback_ttl(mut self, value: Duration) -> Self {
        self.callback_ttl = value;
        self
    }
    pub fn max_callback_references(mut self, value: usize) -> Self {
        self.max_callback_references = value;
        self
    }
    pub fn callback_clock_policy(mut self, value: CallbackClockPolicy) -> Self {
        self.callback_clock_policy = value;
        self
    }
    pub fn max_in_flight(mut self, value: usize) -> Self {
        self.max_in_flight = value;
        self
    }
    pub fn max_read_bytes(mut self, value: usize) -> Self {
        self.max_read_bytes = value;
        self
    }
    pub fn receipt_margin(mut self, value: Duration) -> Self {
        self.receipt_margin = value;
        self
    }
}
struct Profile {
    name: String,
    expected: ProfileIdentity,
    journal: Journal,
    client: tg_mtproto::Client,
    account: tg_mtproto::Account,
    reader: updates::Reader,
}
#[derive(Default)]
struct Jobs {
    closed: bool,
    failed: bool,
    tasks: JoinSet<()>,
}
/// Host-configured MTProto actions. Call `close` before Client/Journal shutdown.
/// Caller cancellation does not cancel a tracked invocation. Last-handle Drop
/// aborts wrappers; already accepted M2 work can finish with an unknown effect.
#[derive(Clone)]
pub struct Executor {
    policy: PolicyStore,
    profiles: Arc<RwLock<BTreeMap<String, Arc<Profile>>>>,
    options: ExecutorOptions,
    slots: Arc<Semaphore>,
    jobs: Arc<Mutex<Jobs>>,
    callback_references: Arc<std::sync::Mutex<callbacks::Registry>>,
    files: Arc<file_store::FileRegistry>,
}
impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoExecutor([REDACTED])")
    }
}
impl Executor {
    pub const ACTIONS: [Action; 8] = Action::ALL;
    pub fn new(policy: PolicyStore, options: ExecutorOptions) -> Result<Self> {
        if !(1..=1024).contains(&options.max_in_flight)
            || !(1..=64 * 1024 * 1024).contains(&options.max_read_bytes)
            || options.receipt_margin.is_zero()
            || options.callback_ttl < Duration::from_millis(1)
            || options.callback_ttl > Duration::from_secs(60)
            || !(1..=65536).contains(&options.max_callback_references)
            || Instant::now().checked_add(options.receipt_margin).is_none()
        {
            return Err(invalid_policy());
        }
        options.file_limits.validate()?;
        Ok(Self {
            policy,
            profiles: Arc::default(),
            slots: Arc::new(Semaphore::new(options.max_in_flight)),
            files: Arc::new(file_store::FileRegistry::new(options.file_limits)),
            options,
            jobs: Arc::default(),
            callback_references: Arc::default(),
        })
    }
    /// Immutable host registration; no login, identity RPC or action is started.
    pub fn register_profile(&self, feed: updates::Feed, client: tg_mtproto::Client) -> Result<()> {
        let jobs = self.jobs.try_lock().map_err(|_| capacity())?;
        if jobs.closed {
            return Err(closed());
        }
        let (journal, name, expected) = feed.executor_binding();
        if client
            .request_budget()
            .checked_add(self.options.receipt_margin)
            .is_none_or(|v| v > journal.lease_duration())
        {
            return Err(invalid_policy());
        }
        let identity = client_identity(expected)?;
        let reader = updates::Reader::new(
            feed.clone(),
            self.policy.clone(),
            updates::ReadOptions::default()
                .max_in_flight(self.options.max_in_flight)
                .max_read_bytes(self.options.max_read_bytes),
        )?;
        let mut profiles = self.profiles.write().map_err(|_| invalid_policy())?;
        if profiles.contains_key(name) || profiles.values().any(|p| p.expected == expected) {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        if profiles.len() >= 64 {
            return Err(capacity());
        }
        profiles.insert(
            name.to_owned(),
            Arc::new(Profile {
                name: name.into(),
                expected,
                journal: journal.clone(),
                account: client.account(identity),
                client,
                reader,
            }),
        );
        Ok(())
    }
    fn worker(&self, request: &Request) -> Result<Worker> {
        supported(request)?;
        let profile = self
            .profiles
            .read()
            .map_err(|_| invalid_policy())?
            .get(request.profile())
            .cloned()
            .ok_or_else(denied)?;
        Ok(Worker {
            profile,
            policy: self.policy.clone(),
            options: self.options.clone(),
            callback_references: self.callback_references.clone(),
            files: self.files.clone(),
        })
    }
    pub async fn dry_run(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<DryRun> {
        let worker = self.worker(request)?;
        if request.action() == Action::ReadUpdates {
            return worker
                .profile
                .reader
                .dry_run(principal, request, approval)
                .await;
        }
        worker
            .authorization(principal, request, approval)
            .await?
            .checked(now()?)
            .map(|(dry, _)| dry)
    }
    pub async fn approve(
        &self,
        principal: &Principal,
        request: &Request,
        expires_at_ms: i64,
    ) -> Result<Approval> {
        let worker = self.worker(request)?;
        if request.action() == Action::ReadUpdates {
            return worker
                .profile
                .reader
                .approve(principal, request, expires_at_ms)
                .await;
        }
        let auth = worker.authorization(principal, request, None).await?;
        if let Some(reference) = &auth.reference {
            let time = now()?;
            let remaining = reference.deadline.remaining(time)?;
            if expires_at_ms > reference.deadline.expires_at_ms
                || expires_at_ms.saturating_sub(time) > remaining.as_millis() as i64
            {
                return Err(invalid_policy());
            }
        }
        let scope = auth.scope.as_ref().map(Scope::policy_scope);
        let approval = self.policy.approve_mtproto_with_scope(
            principal,
            request,
            worker.profile.expected,
            PolicyTime::Current(now()?),
            expires_at_ms,
            scope.as_ref(),
        )?;
        if let Some(reference) = &auth.reference {
            reference.deadline.remaining(now()?)?;
        }
        Ok(approval)
    }
    pub async fn execute(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let result = self.execute_inner(principal, request, approval).await;
        result.map_err(|e| {
            if request.action().is_write() && e.outcome().is_none() {
                e.with_outcome(EffectOutcome::NotSent)
            } else {
                e
            }
        })
    }
    async fn execute_inner(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let worker = self.worker(request)?;
        let handle = tokio::runtime::Handle::try_current().map_err(|_| invalid_policy())?;
        let mut jobs = self.jobs.lock().await;
        if jobs.closed {
            return Err(closed());
        }
        while let Some(result) = jobs.tasks.try_join_next() {
            jobs.failed |= result.is_err();
        }
        if jobs.failed {
            return Err(uncertain());
        }
        if jobs.tasks.len() >= self.options.max_in_flight {
            return Err(capacity());
        }
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| capacity())?;
        let (reply, waiting) = oneshot::channel();
        let (principal, request, approval) =
            (principal.clone(), request.clone(), approval.cloned());
        jobs.tasks.spawn_on(
            async move {
                let _slot = slot;
                let result = worker.run(principal, request, approval).await;
                let _ = reply.send(result);
            },
            &handle,
        );
        drop(jobs);
        let result = waiting.await.map_err(|_| uncertain())??;
        result.disclose()
    }
    /// Stop admission and join every accepted wrapper. Cancelling this wait does
    /// not abort the stored jobs; another close call can resume joining them.
    pub async fn close(&self) -> Result<()> {
        let mut jobs = self.jobs.lock().await;
        jobs.closed = true;
        while let Some(result) = jobs.tasks.join_next().await {
            jobs.failed |= result.is_err();
        }
        if jobs.failed {
            Err(uncertain())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone)]
struct Authorization {
    policy: PolicyStore,
    principal: Principal,
    request: Request,
    approval: Option<Approval>,
    expected: ProfileIdentity,
    scope: Option<Scope>,
    reference: Option<callbacks::Context>,
    file: Option<RegisteredFile>,
}
impl Authorization {
    fn write_checked(&self, time: i64) -> Result<(DryRun, Grant)> {
        let value = self.checked(time)?;
        if let Some(file) = &self.file {
            file_store::ensure_live(file.expires_at_ms, now()?.max(time))?;
        }
        Ok(value)
    }
    fn write_budget(&self, time: i64) -> journal::Result<BudgetContext> {
        let context = self.budget(time)?;
        if let Some(file) = &self.file {
            file_store::ensure_live(
                file.expires_at_ms,
                now()
                    .map_err(|_| JournalError::new(JournalErrorKind::Storage))?
                    .max(time),
            )
            .map_err(|_| JournalError::new(JournalErrorKind::Denied))?;
        }
        Ok(context)
    }
    fn checked(&self, time: i64) -> Result<(DryRun, Grant)> {
        let scope = self.scope.as_ref().map(Scope::policy_scope);
        let result = self.policy.authorized_mtproto(
            &self.principal,
            &self.request,
            self.expected,
            PolicyTime::Current(time),
            self.approval.as_ref(),
            scope.as_ref(),
        )?;
        if let Some(reference) = &self.reference {
            reference.deadline.remaining(now()?.max(time))?;
        }
        Ok(result)
    }
    fn budget(&self, time: i64) -> journal::Result<BudgetContext> {
        let (_, grant) = self
            .checked(time)
            .map_err(|_| JournalError::new(JournalErrorKind::Denied))?;
        Ok(BudgetContext {
            profile: self.request.profile().into(),
            principal: self.principal.as_str().into(),
            cost: grant.cost(self.request.action()),
            budget: grant.budget().clone(),
        })
    }
}
struct Authorized {
    auth: Authorization,
    revision: u64,
    value: ExecutionResult,
}
impl Authorized {
    fn disclose(self) -> Result<ExecutionResult> {
        let result = (|| {
            let (dry, _) = self.auth.checked(now()?)?;
            if dry.grant_revision != self.revision {
                return Err(denied());
            }
            Ok(self.value)
        })();
        result.map_err(|e| {
            if self.auth.request.action().is_write() {
                e.with_outcome(EffectOutcome::Unknown)
            } else {
                e
            }
        })
    }
}
struct Worker {
    profile: Arc<Profile>,
    policy: PolicyStore,
    options: ExecutorOptions,
    callback_references: Arc<std::sync::Mutex<callbacks::Registry>>,
    files: Arc<file_store::FileRegistry>,
}
impl Worker {
    async fn authorization(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<Authorization> {
        let mut reference = None;
        let file = if let Call::SendDocument(args) = request.call() {
            self.policy
                .check_mtproto_lookup(principal, request, self.profile.expected, now()?)?;
            let file = self
                .profile
                .journal
                .file_metadata(request.profile(), principal.as_str(), &args.file_handle)
                .await
                .map_err(storage_error)?
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
            files::validate_metadata(&file)?;
            Some(file)
        } else {
            None
        };
        let scope = if let Call::GetOperation(args) = request.call() {
            self.policy
                .check_mtproto_lookup(principal, request, self.profile.expected, now()?)?;
            Some(
                self.profile
                    .journal
                    .mtproto_operation_scope(
                        request.profile(),
                        principal.as_str(),
                        &args.operation_id,
                    )
                    .await
                    .map_err(storage_error)?
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?,
            )
        } else if let Call::AnswerCallbackQuery(args) = request.call() {
            self.policy
                .check_mtproto_lookup(principal, request, self.profile.expected, now()?)?;
            let found = self
                .callback_references
                .lock()
                .map_err(|_| invalid_policy())?
                .lookup(principal, &self.profile.name, &args.reference, now()?)?;
            let scope = found.context.scope.clone();
            reference = Some(found.context);
            Some(scope)
        } else {
            None
        };
        Ok(Authorization {
            policy: self.policy.clone(),
            principal: principal.clone(),
            request: request.clone(),
            approval: approval.cloned(),
            expected: self.profile.expected,
            scope,
            reference,
            file,
        })
    }
    async fn run(
        &self,
        principal: Principal,
        request: Request,
        approval: Option<Approval>,
    ) -> Result<Authorized> {
        if request.action() == Action::ReadUpdates {
            let auth = self
                .authorization(&principal, &request, approval.as_ref())
                .await?;
            let (revision, mut page) = self
                .profile
                .reader
                .read_with_revision(&principal, &request, approval.as_ref())
                .await?;
            self.attach_callbacks(&principal, &mut page)?;
            return Ok(Authorized {
                auth,
                revision,
                value: ExecutionResult::updates(page),
            });
        }
        let auth = self
            .authorization(&principal, &request, approval.as_ref())
            .await?;
        auth.checked(now()?)?;
        let invocation = auth.clone();
        let expected = self.profile.expected;
        let write = request.action().is_write();
        self.profile
            .journal
            .run(move |c, options, _| {
                charge_invocation_checked(c, options, Some(expected), |time| {
                    let context = invocation.budget(time)?;
                    let cost = if write { 0 } else { context.cost };
                    Ok((context, cost))
                })
            })
            .await
            .map_err(storage_error)?;
        auth.checked(now()?)?;
        let value = match request.call() {
            Call::GetMe(_) => {
                self.profile
                    .client
                    .verify_account(client_identity(self.profile.expected)?)
                    .await
                    .map_err(remote)?;
                ExecutionResult::account(self.profile.expected)
            }
            Call::GetPeer(args) => {
                let peer = self
                    .profile
                    .account
                    .cached_peer(args.peer.as_client_id())
                    .await
                    .map_err(remote)?;
                ExecutionResult::peer(args.peer, peer.is_some())
            }
            Call::GetOperation(args) => {
                let (operation, scope) = self
                    .profile
                    .journal
                    .get_mtproto_scoped(request.profile(), principal.as_str(), &args.operation_id)
                    .await
                    .map_err(storage_error)?
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
                if auth.scope.as_ref() != Some(&scope) {
                    return Err(denied());
                }
                output::project(operation, &scope)?
            }
            Call::SendMessage(_)
            | Call::EditMessageText(_)
            | Call::AnswerCallbackQuery(_)
            | Call::SendDocument(_) => self.write(&auth).await?,
            _ => return Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction)),
        };
        let (dry, _) = auth.checked(now()?).map_err(|e| {
            if write {
                e.with_outcome(EffectOutcome::Unknown)
            } else {
                e
            }
        })?;
        Ok(Authorized {
            auth,
            revision: dry.grant_revision,
            value,
        })
    }
}
fn supported(request: &Request) -> Result<()> {
    if Executor::ACTIONS.contains(&request.action()) {
        Ok(())
    } else {
        Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction))
    }
}
fn client_identity(expected: ProfileIdentity) -> Result<tg_mtproto::AccountIdentity> {
    Ok(tg_mtproto::AccountIdentity {
        id: tg_mtproto::AccountId::new(expected.account_id()).map_err(|_| invalid_policy())?,
        kind: match expected.kind() {
            ProfileKind::MtprotoUser => tg_mtproto::AccountKind::User,
            ProfileKind::MtprotoBot => tg_mtproto::AccountKind::Bot,
            _ => return Err(invalid_policy()),
        },
    })
}
fn now() -> Result<i64> {
    journal::now().map_err(storage_error)
}
fn denied() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Denied)
}
fn invalid_policy() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
}
fn capacity() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Capacity)
}
fn closed() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Closed)
}
fn uncertain() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Uncertain).with_outcome(EffectOutcome::Unknown)
}
fn remote(error: tg_mtproto::ClientError) -> ExecutorError {
    ExecutorError::new(match error.kind() {
        tg_mtproto::ClientErrorKind::Capacity => ExecutorErrorKind::Capacity,
        tg_mtproto::ClientErrorKind::Closed => ExecutorErrorKind::Closed,
        tg_mtproto::ClientErrorKind::IdentityMismatch
        | tg_mtproto::ClientErrorKind::Quarantined => ExecutorErrorKind::Identity,
        tg_mtproto::ClientErrorKind::PeerUnavailable => ExecutorErrorKind::NotFound,
        _ => ExecutorErrorKind::Remote,
    })
}
