use super::{policy::*, types::*};
use crate::{
    journal::{
        self, AttemptFailure, Completion, Journal, JournalError, JournalErrorKind, Operation,
        OperationRequest, OperationState, Preparation, ResultAvailability,
        authorization::{BudgetContext, OperationScope},
    },
    updates::UpdateFeed,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};
use tg::{Bot, EffectOutcome};
use tokio::{
    sync::{OnceCell, Semaphore},
    time::{Instant, timeout, timeout_at},
};

#[derive(Debug, Clone)]
pub struct ExecutorOptions {
    max_in_flight: usize,
    request_budget: Duration,
    receipt_margin: Duration,
}
impl Default for ExecutorOptions {
    fn default() -> Self {
        Self {
            max_in_flight: 16,
            request_budget: Duration::from_secs(30),
            receipt_margin: Duration::from_secs(5),
        }
    }
}
impl ExecutorOptions {
    pub fn max_in_flight(mut self, value: usize) -> Self {
        self.max_in_flight = value;
        self
    }
    pub fn request_budget(mut self, value: Duration) -> Self {
        self.request_budget = value;
        self
    }
    pub fn receipt_margin(mut self, value: Duration) -> Self {
        self.receipt_margin = value;
        self
    }
    fn validate(&self) -> Result<()> {
        if self.max_in_flight == 0
            || self.max_in_flight > 1024
            || self.request_budget.is_zero()
            || self.receipt_margin.is_zero()
            || self
                .request_budget
                .checked_add(self.receipt_margin)
                .and_then(|value| Instant::now().checked_add(value))
                .is_none()
        {
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy));
        }
        Ok(())
    }
}
struct Profile {
    bot: Bot,
    journal: Journal,
    bot_id: i64,
    identity: OnceCell<()>,
}
/// Profiles and authenticated authority are host configuration, never tool args.
#[derive(Clone)]
pub struct Executor {
    policy: PolicyStore,
    profiles: Arc<RwLock<BTreeMap<String, Arc<Profile>>>>,
    slots: Arc<Semaphore>,
    options: ExecutorOptions,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReceipt {
    pub chat_id: ChatId,
    pub message_id: MessageId,
}
#[derive(Serialize)]
pub struct OperationReceipt {
    operation_id: String,
    action: Action,
    state: OperationState,
    result_availability: ResultAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<MessageReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<FailureReceipt>,
}
/// Allowlisted failure advice, separate from internal journal serialization.
#[derive(Serialize)]
pub struct FailureReceipt {
    kind: &'static str,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_error_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    migrate_to_chat_id: Option<ChatId>,
}
impl FailureReceipt {
    pub fn kind(&self) -> &'static str {
        self.kind
    }
    pub fn outcome(&self) -> &'static str {
        self.outcome
    }
    pub fn migrate_to_chat_id(&self) -> Option<ChatId> {
        self.migrate_to_chat_id
    }
    fn project(value: &AttemptFailure) -> Self {
        let kind = match value.kind() {
            "configuration" => "configuration",
            "validation" => "validation",
            "capacity" => "capacity",
            "io" => "io",
            "timeout" => "timeout",
            "transport" => "transport",
            "http" => "http",
            "telegram" => "telegram",
            "decode" => "decode",
            "response_too_large" => "response_too_large",
            "handler" => "handler",
            "executor" => "executor",
            _ => "other",
        };
        let outcome = match value.outcome() {
            "not_sent" => "not_sent",
            "rejected" => "rejected",
            _ => "unknown",
        };
        Self {
            kind,
            outcome,
            http_status: value.http_status(),
            api_error_code: value.api_error_code(),
            retry_after_secs: value.retry_after().map(|v| v.as_secs()),
            migrate_to_chat_id: value
                .migrate_to_chat_id()
                .and_then(|v| ChatId::new(i64::from(v)).ok()),
        }
    }
}
impl OperationReceipt {
    pub fn state(&self) -> OperationState {
        self.state
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn message(&self) -> Option<&MessageReceipt> {
        self.message.as_ref()
    }
    pub fn result_availability(&self) -> ResultAvailability {
        self.result_availability
    }
    pub fn failure(&self) -> Option<&FailureReceipt> {
        self.failure.as_ref()
    }
}
#[derive(Serialize)]
pub struct ExecutionResult {
    schema_version: u32,
    operation: OperationReceipt,
}
impl ExecutionResult {
    pub fn operation(&self) -> Option<&OperationReceipt> {
        Some(&self.operation)
    }
}
impl Executor {
    pub fn new(policy: PolicyStore, options: ExecutorOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            policy,
            profiles: Arc::default(),
            slots: Arc::new(Semaphore::new(options.max_in_flight)),
            options,
        })
    }
    /// Immutable registration. UpdateFeed permanently binds the profile's bot ID
    /// and owns its journal; first authorized remote use verifies this Bot lazily.
    pub fn register_profile(&self, feed: UpdateFeed, bot: Bot) -> Result<()> {
        let (journal, name, bot_id) = feed.executor_binding();
        valid_id(name)?;
        if self
            .options
            .request_budget
            .checked_add(self.options.receipt_margin)
            .is_none_or(|v| v > journal.lease_duration())
        {
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy));
        }
        let mut profiles = self
            .profiles
            .write()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?;
        if profiles.contains_key(name) || profiles.values().any(|v| v.bot_id == bot_id) {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        if profiles.len() >= 64 {
            return Err(ExecutorError::new(ExecutorErrorKind::Capacity));
        }
        profiles.insert(
            name.to_owned(),
            Arc::new(Profile {
                bot,
                journal: journal.clone(),
                bot_id,
                identity: OnceCell::new(),
            }),
        );
        Ok(())
    }
    /// Experimental A1 write slice. Only send_message is executable so far;
    /// other typed actions fail locally until their scope/receipt integration lands.
    pub async fn execute(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let result = self.execute_inner(principal, request, approval).await;
        result.map_err(|error| {
            if request.action().is_write() && error.outcome().is_none() {
                error.with_outcome(EffectOutcome::NotSent)
            } else {
                error
            }
        })
    }
    async fn execute_inner(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let ActionCall::SendMessage(args) = request.call() else {
            return Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction));
        };
        let _slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::Capacity))?;
        let profile = self
            .profiles
            .read()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .get(request.profile())
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Denied))?;
        self.policy
            .authorized(principal, request, profile.bot_id, now()?, approval)?;
        let invocation = Authorization {
            policy: self.policy.clone(),
            principal: principal.clone(),
            request: request.clone(),
            approval: approval.cloned(),
            bot_id: profile.bot_id,
        };
        let admission = invocation.clone();
        profile
            .journal
            .charge_authorized_invocation(move |time| Ok((invocation.budget_at(time)?, 0)))
            .await
            .map_err(storage_error)?;
        self.policy
            .dry_run(principal, request, profile.bot_id, now()?, approval)?;
        let scope = OperationScope {
            bot_id: profile.bot_id,
            action: Action::SendMessage,
            chat: Some(args.chat_id),
            file: None,
            reference_hash: None,
        };
        let intent = OperationRequest::new(
            request.profile(),
            principal.as_str(),
            request.operation_id().expect("validated write ID"),
            request.action().name(),
            &serde_json::to_value(request)
                .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidInput))?,
        )
        .map_err(storage_error)?;
        let prepared = match profile
            .journal
            .prepare_scoped(intent, scope.clone())
            .await
            .map_err(storage_error)?
        {
            Preparation::Ready(value) => value,
            Preparation::Existing(_) => {
                return self
                    .cached(&profile, principal, request, approval, &scope)
                    .await
                    .map_err(|error| error.with_outcome(EffectOutcome::Unknown));
            }
        };
        let identity = profile
            .identity
            .get_or_try_init(|| async {
                self.policy
                    .dry_run(principal, request, profile.bot_id, now()?, approval)?;
                let identity =
                    timeout(self.options.request_budget, profile.bot.verified_identity())
                        .await
                        .map_err(|_| ExecutorError::new(ExecutorErrorKind::Identity))?
                        .map_err(|_| ExecutorError::new(ExecutorErrorKind::Identity))?;
                if i64::from(identity.id) != profile.bot_id {
                    return Err(ExecutorError::new(ExecutorErrorKind::Identity));
                }
                Ok(())
            })
            .await;
        if let Err(error) = identity {
            let _ = profile.journal.release_prepared(prepared).await;
            return Err(error);
        }
        let proof = prepared.clone();
        let flight = profile
            .journal
            .begin_authorized(prepared, move |time| admission.budget_at(time))
            .await;
        let flight = match flight {
            Ok(value) => value,
            Err(error) => {
                let _ = profile.journal.release_prepared(proof).await;
                return Err(storage_error(error));
            }
        };
        let remaining = flight
            .lease_until_ms()
            .checked_sub(now()?)
            .and_then(|v| u64::try_from(v).ok())
            .map(Duration::from_millis)
            .unwrap_or_default();
        let rpc_budget = remaining
            .checked_sub(self.options.receipt_margin)
            .filter(|v| !v.is_zero())
            .map(|v| v.min(self.options.request_budget));
        if let Err(error) =
            self.policy
                .dry_run(principal, request, profile.bot_id, now()?, approval)
        {
            let _ = profile
                .journal
                .complete_fenced(flight, Completion::executor_not_started())
                .await;
            return Err(error.with_outcome(EffectOutcome::NotSent));
        }
        let Some(rpc_budget) = rpc_budget else {
            let _ = profile
                .journal
                .complete_fenced(flight, Completion::executor_not_started())
                .await;
            return Err(ExecutorError::new(ExecutorErrorKind::Capacity)
                .with_outcome(EffectOutcome::NotSent));
        };
        let Some(end) = Instant::now().checked_add(rpc_budget + self.options.receipt_margin) else {
            let _ = profile
                .journal
                .complete_fenced(flight, Completion::executor_not_started())
                .await;
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
                .with_outcome(EffectOutcome::NotSent));
        };
        let mut params = tg::api::methods::SendMessage::new(args.text.clone())
            .chat_id(tg::ChatId::from(args.chat_id.get()).into());
        if let Some(id) = args.message_thread_id {
            params = params.message_thread_id(id.get());
        }
        if let Some(value) = args.disable_notification {
            params = params.disable_notification(value);
        }
        let completion = match timeout(
            rpc_budget,
            profile.bot.call(params).retry_policy(
                tg::RetryPolicy::default()
                    .max_attempts(1)
                    .max_elapsed(rpc_budget),
            ),
        )
        .await
        {
            Ok(Ok(message))
                if i64::from(message.chat.id) == args.chat_id.get()
                    && i64::from(message.message_id) > 0 =>
            {
                Completion::succeeded(
                    serde_json::json!({"chat_id":args.chat_id,"message_id":i64::from(message.message_id).to_string()}),
                )
            }
            Ok(Err(error)) => Completion::from_error(&error),
            _ => Completion::executor_uncertain(),
        };
        timeout_at(end, profile.journal.complete_fenced(flight, completion))
            .await
            .map_err(|_| uncertain())?
            .map_err(|_| uncertain())?;
        self.cached(&profile, principal, request, approval, &scope)
            .await
            .map_err(|error| error.with_outcome(EffectOutcome::Unknown))
    }
    async fn cached(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
        expected: &OperationScope,
    ) -> Result<ExecutionResult> {
        let (operation, scope) = profile
            .journal
            .get_scoped(
                request.profile(),
                principal.as_str(),
                request.operation_id().expect("validated write ID"),
            )
            .await
            .map_err(storage_error)?
            .ok_or_else(uncertain)?;
        self.policy
            .dry_run(principal, request, profile.bot_id, now()?, approval)?;
        if scope != *expected {
            return Err(ExecutorError::new(ExecutorErrorKind::Denied));
        }
        project(operation, &scope)
    }
}
#[derive(Clone)]
struct Authorization {
    policy: PolicyStore,
    principal: Principal,
    request: Request,
    approval: Option<Approval>,
    bot_id: i64,
}
impl Authorization {
    fn budget_at(&self, time: i64) -> journal::Result<BudgetContext> {
        let (_, grant) = self
            .policy
            .authorized(
                &self.principal,
                &self.request,
                self.bot_id,
                time,
                self.approval.as_ref(),
            )
            .map_err(|_| JournalError::new(JournalErrorKind::Denied))?;
        Ok(budget_context(&self.principal, &self.request, &grant))
    }
}
fn budget_context(principal: &Principal, request: &Request, grant: &Grant) -> BudgetContext {
    BudgetContext {
        profile: request.profile().into(),
        principal: principal.as_str().into(),
        budget: grant.budget().clone(),
        cost: grant.cost(request.action()),
    }
}
fn now() -> Result<i64> {
    journal::now().map_err(storage_error)
}
fn storage_error(error: JournalError) -> ExecutorError {
    ExecutorError::new(match error.kind() {
        JournalErrorKind::Validation => ExecutorErrorKind::InvalidInput,
        JournalErrorKind::Denied => ExecutorErrorKind::Denied,
        JournalErrorKind::Conflict => ExecutorErrorKind::Conflict,
        JournalErrorKind::QuotaExceeded => ExecutorErrorKind::QuotaExceeded,
        JournalErrorKind::Capacity => ExecutorErrorKind::Capacity,
        _ => ExecutorErrorKind::Storage,
    })
}
fn uncertain() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Uncertain).with_outcome(EffectOutcome::Unknown)
}
fn project(operation: Operation, scope: &OperationScope) -> Result<ExecutionResult> {
    let message = operation
        .result()
        .map(|value| {
            serde_json::from_value::<MessageReceipt>(value.clone()).map_err(|_| uncertain())
        })
        .transpose()?;
    if message
        .as_ref()
        .is_some_and(|v| Some(v.chat_id) != scope.chat)
    {
        return Err(uncertain());
    }
    Ok(ExecutionResult {
        schema_version: 1,
        operation: OperationReceipt {
            operation_id: operation.operation_id().into(),
            action: scope.action,
            state: operation.state(),
            result_availability: operation.result_availability(),
            message,
            failure: operation.failure().map(FailureReceipt::project),
        },
    })
}
macro_rules! redacted_debug {
    ($($name:ident),+ $(,)?) => { $(impl fmt::Debug for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name),"([REDACTED])")) } })+ };
}
redacted_debug!(
    Executor,
    ExecutionResult,
    OperationReceipt,
    MessageReceipt,
    FailureReceipt
);
