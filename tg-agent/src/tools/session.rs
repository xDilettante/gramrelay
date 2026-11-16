use crate::executor::{
    Action, Approval, DryRun, ExecutionResult, Executor, ExecutorError, Principal, Request,
};
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, Copy)]
pub enum ExecutionMode {
    Execute,
    DryRun,
}
#[derive(Clone)]
pub struct ToolSession {
    executor: Executor,
    principal: Principal,
    approvals: Arc<RwLock<BTreeMap<[u8; 32], Approval>>>,
}
impl ToolSession {
    pub fn new(executor: Executor, principal: Principal) -> Self {
        Self {
            executor,
            principal,
            approvals: Default::default(),
        }
    }
    /// Trusted host confirmation, never an exposed tool or input field.
    pub async fn approve(&self, request: &Request, expires_at_ms: i64) -> Result<(), ToolError> {
        let approval = self
            .executor
            .approve(&self.principal, request, expires_at_ms)
            .await
            .map_err(|error| ToolError::from_executor(&error, request))?;
        let mut approvals = self.approvals.write().map_err(|_| ToolError::session())?;
        let key = request.fingerprint();
        if approvals.len() >= 256 && !approvals.contains_key(&key) {
            return Err(ToolError::new(
                "agent_capacity",
                "Host approval capacity reached.",
            ));
        }
        approvals.insert(key, approval);
        Ok(())
    }
    /// Removes saved approvals for subsequent lookups. Change/revoke the policy
    /// grant to invalidate a capability already supplied to an in-progress call.
    pub fn clear_approvals(&self) -> Result<(), ToolError> {
        self.approvals
            .write()
            .map_err(|_| ToolError::session())?
            .clear();
        Ok(())
    }
    pub async fn dispatch_json(&self, input: &[u8], mode: ExecutionMode) -> ToolResponse {
        match Request::from_json(input) {
            Ok(request) => self.execute_for(request.action(), &request, mode).await,
            Err(_) => ToolResponse::Error(ToolError::invalid_input()),
        }
    }
    /// Bind an adapter-selected name independently of model-provided arguments.
    pub async fn execute_for(
        &self,
        selected: Action,
        request: &Request,
        mode: ExecutionMode,
    ) -> ToolResponse {
        if selected != request.action() {
            return ToolResponse::Error(
                ToolError::new(
                    "agent_tool_mismatch",
                    "Selected tool does not match the request.",
                )
                .with_request(request),
            );
        }
        let approval = match self.approvals.read() {
            Ok(approvals) => approvals.get(&request.fingerprint()).cloned(),
            Err(_) => return ToolResponse::Error(ToolError::session().with_request(request)),
        };
        match mode {
            ExecutionMode::Execute => match self
                .executor
                .execute(&self.principal, request, approval.as_ref())
                .await
            {
                Ok(value) => ToolResponse::Success(value),
                Err(error) => ToolResponse::Error(ToolError::from_executor(&error, request)),
            },
            ExecutionMode::DryRun => match self
                .executor
                .dry_run(&self.principal, request, approval.as_ref())
                .await
            {
                Ok(value) => ToolResponse::DryRun(value),
                Err(error) => ToolResponse::Error(ToolError::from_executor(&error, request)),
            },
        }
    }
}
#[derive(Clone, Serialize)]
pub struct ToolError {
    code: &'static str,
    message: &'static str,
    automatic_retry_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action: Option<&'static str>,
}
impl ToolError {
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub(crate) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            automatic_retry_allowed: false,
            effect_outcome: None,
            operation_id: None,
            next_action: None,
        }
    }
    pub(crate) fn invalid_input() -> Self {
        Self::new("executor_invalid_input", "Invalid versioned tool request.")
    }
    fn session() -> Self {
        Self::new("agent_session", "Host session is unavailable.")
    }
    fn from_executor(error: &ExecutorError, request: &Request) -> Self {
        let mut result = Self::new(
            error.code(),
            "The request did not return a result. Inspect its code and original operation.",
        );
        result.effect_outcome = error.outcome().map(|outcome| match outcome {
            tg::EffectOutcome::NotSent => "not_sent",
            tg::EffectOutcome::Rejected => "rejected",
            _ => "unknown",
        });
        result.bind_request(request);
        result
    }
    fn bind_request(&mut self, request: &Request) {
        if request.action().is_write() {
            self.operation_id = request.operation_id().map(str::to_owned);
            self.next_action = Some("get_operation");
        }
    }
    fn with_request(mut self, request: &Request) -> Self {
        self.bind_request(request);
        self
    }
    pub(crate) fn output_too_large(request: Option<&Request>) -> Self {
        Self::delivery_error(
            "agent_output_too_large",
            "Response exceeded the output limit.",
            request,
        )
    }
    #[cfg(feature = "cli")]
    pub(crate) fn output_io(request: Option<&Request>) -> Self {
        Self::delivery_error(
            "agent_io",
            "JSON transport failed. Recover writes using their original operation keys.",
            request,
        )
    }
    fn delivery_error(
        code: &'static str,
        message: &'static str,
        request: Option<&Request>,
    ) -> Self {
        let mut result = Self::new(code, message);
        if let Some(request) = request {
            result.bind_request(request);
            if request.action().is_write() {
                result.effect_outcome = Some("unknown");
            }
        }
        result
    }
}
impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for ToolError {}
pub enum ToolResponse {
    Success(ExecutionResult),
    DryRun(DryRun),
    Error(ToolError),
}
impl ToolResponse {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}
impl Serialize for ToolResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("schema_version", &1u32)?;
        map.serialize_entry("ok", &!self.is_error())?;
        match self {
            Self::Success(value) => map.serialize_entry("result", value)?,
            Self::DryRun(value) => map.serialize_entry("dry_run", value)?,
            Self::Error(value) => map.serialize_entry("error", value)?,
        }
        map.end()
    }
}
macro_rules! redacted {($($type:ident),+)=>{$(impl fmt::Debug for $type {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str(concat!(stringify!($type),"([REDACTED])"))}})+};}
redacted!(ToolSession, ToolError, ToolResponse);
