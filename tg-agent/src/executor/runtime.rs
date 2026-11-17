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
mod callbacks;
mod files;
mod read;
#[cfg(feature = "host")]
pub(crate) use read::validate_cursor;
mod write;
pub use callbacks::{CallbackClockPolicy, CallbackReference};
use callbacks::{CallbackRegistry, ReferenceDeadline};
pub use files::{FileLimits, RegisteredFile};
pub use read::{BotInfo, ChatInfo, ReadEvent, ReadEventScope, ReadPage};

#[derive(Debug, Clone)]
pub struct ExecutorOptions {
    max_in_flight: usize,
    request_budget: Duration,
    receipt_margin: Duration,
    max_read_bytes: usize,
    callback_ttl: Duration,
    max_callback_references: usize,
    callback_clock_policy: CallbackClockPolicy,
    file_limits: FileLimits,
}
impl Default for ExecutorOptions {
    fn default() -> Self {
        Self {
            max_in_flight: 16,
            request_budget: Duration::from_secs(30),
            receipt_margin: Duration::from_secs(5),
            max_read_bytes: 1024 * 1024,
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
    pub fn request_budget(mut self, value: Duration) -> Self {
        self.request_budget = value;
        self
    }
    pub fn receipt_margin(mut self, value: Duration) -> Self {
        self.receipt_margin = value;
        self
    }
    pub fn max_read_bytes(mut self, value: usize) -> Self {
        self.max_read_bytes = value;
        self
    }
    fn validate(&self) -> Result<()> {
        self.file_limits.validate()?;
        if self.max_in_flight == 0
            || self.max_in_flight > 1024
            || self.request_budget.is_zero()
            || self.receipt_margin.is_zero()
            || self.max_read_bytes == 0
            || self.max_read_bytes > 64 * 1024 * 1024
            || self.callback_ttl < Duration::from_millis(1)
            || self.callback_ttl > Duration::from_secs(60)
            || self.max_callback_references == 0
            || self.max_callback_references > 65536
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
    expected: crate::profiles::ProfileIdentity,
    journal: Journal,
    bot_id: i64,
    identity: OnceCell<BotInfo>,
    feed: UpdateFeed,
}
/// Profiles and authenticated authority are host configuration, never tool args.
#[derive(Clone)]
pub struct Executor {
    policy: PolicyStore,
    profiles: Arc<RwLock<BTreeMap<String, Arc<Profile>>>>,
    slots: Arc<Semaphore>,
    options: ExecutorOptions,
    callback_references: Arc<std::sync::Mutex<CallbackRegistry>>,
    files: Arc<files::FileRegistry>,
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
    acknowledged: Option<bool>,
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
    pub fn acknowledged(&self) -> Option<bool> {
        self.acknowledged
    }
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
    #[serde(flatten)]
    value: ExecutionValue,
}
#[derive(Serialize)]
#[serde(untagged)]
enum ExecutionValue {
    Operation { operation: OperationReceipt },
    Bot { bot: BotInfo },
    Chat { chat: ChatInfo },
    Updates { updates: ReadPage },
}
impl ExecutionResult {
    pub fn operation(&self) -> Option<&OperationReceipt> {
        match &self.value {
            ExecutionValue::Operation { operation } => Some(operation),
            _ => None,
        }
    }
    pub fn bot(&self) -> Option<&BotInfo> {
        match &self.value {
            ExecutionValue::Bot { bot } => Some(bot),
            _ => None,
        }
    }
    pub fn chat(&self) -> Option<&ChatInfo> {
        match &self.value {
            ExecutionValue::Chat { chat } => Some(chat),
            _ => None,
        }
    }
    pub fn updates(&self) -> Option<&ReadPage> {
        match &self.value {
            ExecutionValue::Updates { updates } => Some(updates),
            _ => None,
        }
    }
}
impl Executor {
    pub fn new(policy: PolicyStore, options: ExecutorOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            policy,
            profiles: Arc::default(),
            slots: Arc::new(Semaphore::new(options.max_in_flight)),
            files: Arc::new(files::FileRegistry::new(options.file_limits)),
            options,
            callback_references: Arc::default(),
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
                expected: crate::profiles::ProfileIdentity::bot_api(bot_id)
                    .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?,
                journal: journal.clone(),
                bot_id,
                identity: OnceCell::new(),
                feed: feed.clone(),
            }),
        );
        Ok(())
    }
    /// Shared authorized execution through current policy and durable admission.
    pub async fn execute(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let result = self.execute_inner(principal, request, approval).await;
        result.map_err(|error| {
            if request.action().is_write() && error.outcome().is_none() {
                error.with_outcome(if request.action() == Action::AnswerCallbackQuery {
                    EffectOutcome::Unknown
                } else {
                    EffectOutcome::NotSent
                })
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
        supported(request)?;
        let _slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::Capacity))?;
        let profile = self.profile_for(request)?;
        if request.action().is_write() {
            self.execute_write(&profile, principal, request, approval)
                .await
        } else {
            self.execute_read(&profile, principal, request, approval)
                .await
        }
    }
    fn profile_for(&self, request: &Request) -> Result<Arc<Profile>> {
        self.profiles
            .read()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .get(request.profile())
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Denied))
    }
    async fn cached(
        &self,
        profile: &Profile,
        auth: &Authorization,
        expected: &OperationScope,
    ) -> Result<ExecutionResult> {
        let (principal, request) = (&auth.principal, &auth.request);
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
        auth.checked(now()?)?;
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
    scope: Option<OperationScope>,
    reference: Option<ReferenceDeadline>,
    file: Option<RegisteredFile>,
}
impl Authorization {
    fn checked(&self, time: i64) -> Result<(DryRun, Grant)> {
        let result = self.policy_checked(time)?;
        if let Some(reference) = &self.reference {
            reference.remaining(time)?;
        }
        Ok(result)
    }
    fn policy_checked(&self, time: i64) -> Result<(DryRun, Grant)> {
        self.policy.authorized_with_scope(
            &self.principal,
            &self.request,
            self.bot_id,
            time,
            self.approval.as_ref(),
            self.scope.as_ref(),
        )
    }
    fn budget_at(&self, time: i64) -> journal::Result<BudgetContext> {
        let (_, grant) = self
            .checked(time)
            .map_err(|_| JournalError::new(JournalErrorKind::Denied))?;
        Ok(budget_context(&self.principal, &self.request, &grant))
    }
    fn write_checked(&self, time: i64) -> Result<(DryRun, Grant)> {
        let result = self.checked(time)?;
        if let Some(file) = &self.file {
            files::ensure_live(file.expires_at_ms, now()?)?;
        }
        Ok(result)
    }
    fn write_budget_at(&self, time: i64) -> journal::Result<BudgetContext> {
        let (_, grant) = self
            .write_checked(time)
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
        JournalErrorKind::CursorExpired => ExecutorErrorKind::CursorExpired,
        _ => ExecutorErrorKind::Storage,
    })
}
fn uncertain() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Uncertain).with_outcome(EffectOutcome::Unknown)
}
fn project(operation: Operation, scope: &OperationScope) -> Result<ExecutionResult> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Acknowledgement {
        acknowledged: tg::api::True,
    }
    let (message, acknowledged) = if scope.action == Action::AnswerCallbackQuery {
        let acknowledged = operation
            .result()
            .map(|value| {
                serde_json::from_value::<Acknowledgement>(value.clone())
                    .map(|value| {
                        let _ = value.acknowledged;
                        true
                    })
                    .map_err(|_| uncertain())
            })
            .transpose()?;
        (None, acknowledged)
    } else {
        (
            operation
                .result()
                .map(|value| {
                    serde_json::from_value::<MessageReceipt>(value.clone()).map_err(|_| uncertain())
                })
                .transpose()?,
            None,
        )
    };
    if message
        .as_ref()
        .is_some_and(|v| Some(v.chat_id) != scope.chat)
    {
        return Err(uncertain());
    }
    Ok(ExecutionResult {
        schema_version: 1,
        value: ExecutionValue::Operation {
            operation: OperationReceipt {
                operation_id: operation.operation_id().into(),
                action: scope.action,
                state: operation.state(),
                result_availability: operation.result_availability(),
                message,
                acknowledged,
                failure: operation.failure().map(FailureReceipt::project),
            },
        },
    })
}
fn supported(request: &Request) -> Result<()> {
    if matches!(
        request.action(),
        Action::GetMe
            | Action::GetChat
            | Action::ReadUpdates
            | Action::GetOperation
            | Action::SendDocument
            | Action::SendMessage
            | Action::EditMessageText
            | Action::AnswerCallbackQuery
    ) {
        Ok(())
    } else {
        Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction))
    }
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
