use super::{ToolCatalog, ToolRequest};
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
    #[cfg(feature = "mtproto")]
    mtproto: Option<crate::mtproto::Executor>,
}
impl ToolSession {
    pub fn new(executor: Executor, principal: Principal) -> Self {
        Self {
            executor,
            principal,
            approvals: Default::default(),
            #[cfg(feature = "mtproto")]
            mtproto: None,
        }
    }
    /// Trusted host attachment. No request can enable this provider.
    #[cfg(feature = "mtproto")]
    pub fn with_mtproto(mut self, executor: crate::mtproto::Executor) -> Self {
        self.mtproto = Some(executor);
        self
    }
    pub fn catalog(&self) -> ToolCatalog {
        #[cfg(feature = "mtproto")]
        if self.mtproto.is_some() {
            return super::extended_catalog();
        }
        super::catalog()
    }
    /// Trusted host confirmation, never an exposed tool or input field.
    pub async fn approve(&self, request: &Request, expires_at_ms: i64) -> Result<(), ToolError> {
        let approval = self
            .executor
            .approve(&self.principal, request, expires_at_ms)
            .await
            .map_err(|error| ToolError::from_executor(&error, request))?;
        self.save_approval(request.fingerprint(), approval)
    }
    fn save_approval(&self, key: [u8; 32], approval: Approval) -> Result<(), ToolError> {
        let mut approvals = self.approvals.write().map_err(|_| ToolError::session())?;
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
        match ToolRequest::from_json(input) {
            Ok(request) => self.execute_named(request.name(), &request, mode).await,
            Err(_) => ToolResponse::Error(ToolError::invalid_input()),
        }
    }
    /// Binds an adapter-selected tool name before either provider is entered.
    pub async fn execute_named(
        &self,
        selected: &str,
        request: &ToolRequest,
        mode: ExecutionMode,
    ) -> ToolResponse {
        if selected != request.name() {
            return ToolResponse::Error(
                ToolError::new(
                    "agent_tool_mismatch",
                    "Selected tool does not match the request.",
                )
                .with_tool_request(request),
            );
        }
        match request {
            ToolRequest::BotApi(request) => self.execute_for(request.action(), request, mode).await,
            #[cfg(feature = "mtproto")]
            ToolRequest::Mtproto(request) => self.execute_mtproto(request, mode).await,
        }
    }
    /// Exact MT confirmation supplied only by the trusted host.
    #[cfg(feature = "mtproto")]
    pub async fn approve_mtproto(
        &self,
        request: &crate::mtproto::Request,
        expires_at_ms: i64,
    ) -> Result<(), ToolError> {
        let executor = self.mtproto.as_ref().ok_or_else(protocol_unavailable)?;
        let approval = executor
            .approve(&self.principal, request, expires_at_ms)
            .await
            .map_err(|error| ToolError::from_mtproto(&error, request))?;
        self.save_approval(request.fingerprint(), approval)
    }
    #[cfg(feature = "mtproto")]
    async fn execute_mtproto(
        &self,
        request: &crate::mtproto::Request,
        mode: ExecutionMode,
    ) -> ToolResponse {
        let Some(executor) = &self.mtproto else {
            return ToolResponse::Error(protocol_unavailable().with_mtproto_request(request));
        };
        let approval = match self.approvals.read() {
            Ok(approvals) => approvals.get(&request.fingerprint()).cloned(),
            Err(_) => {
                return ToolResponse::Error(ToolError::session().with_mtproto_request(request));
            }
        };
        match mode {
            ExecutionMode::Execute => match executor
                .execute(&self.principal, request, approval.as_ref())
                .await
            {
                Ok(value) => ToolResponse::MtprotoSuccess(value),
                Err(error) => ToolResponse::Error(ToolError::from_mtproto(&error, request)),
            },
            ExecutionMode::DryRun => match executor
                .dry_run(&self.principal, request, approval.as_ref())
                .await
            {
                Ok(value) => ToolResponse::MtprotoDryRun(value),
                Err(error) => ToolResponse::Error(ToolError::from_mtproto(&error, request)),
            },
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
        let mut result = Self::executor_error(error);
        result.bind_request(request);
        result
    }
    fn executor_error(error: &ExecutorError) -> Self {
        let mut result = Self::new(
            error.code(),
            "The request did not return a result. Inspect its code and original operation.",
        );
        result.effect_outcome = error.outcome().map(|outcome| match outcome {
            tg::EffectOutcome::NotSent => "not_sent",
            tg::EffectOutcome::Rejected => "rejected",
            _ => "unknown",
        });
        result
    }
    fn bind_request(&mut self, request: &Request) {
        self.bind(
            request.action().is_write(),
            request.operation_id(),
            "get_operation",
        );
    }
    fn bind(&mut self, is_write: bool, operation_id: Option<&str>, recovery: &'static str) {
        if is_write {
            self.operation_id = operation_id.map(str::to_owned);
            self.next_action = Some(recovery);
        }
    }
    #[cfg(feature = "mtproto")]
    fn from_mtproto(error: &ExecutorError, request: &crate::mtproto::Request) -> Self {
        Self::executor_error(error).with_mtproto_request(request)
    }
    #[cfg(feature = "mtproto")]
    fn with_mtproto_request(mut self, request: &crate::mtproto::Request) -> Self {
        self.bind(
            request.action().is_write(),
            request.operation_id(),
            "mtproto_get_operation",
        );
        self
    }
    fn with_tool_request(mut self, request: &ToolRequest) -> Self {
        self.bind(
            request.is_write(),
            request.operation_id(),
            request.recovery_tool(),
        );
        self
    }
    pub(crate) fn tool_output_too_large(request: Option<&ToolRequest>) -> Self {
        Self::tool_delivery_error(
            "agent_output_too_large",
            "Response exceeded the output limit.",
            request,
        )
    }
    #[cfg(feature = "cli")]
    pub(crate) fn tool_output_io(request: Option<&ToolRequest>) -> Self {
        Self::tool_delivery_error(
            "agent_io",
            "JSON transport failed. Recover writes using their original operation keys.",
            request,
        )
    }
    fn tool_delivery_error(
        code: &'static str,
        message: &'static str,
        request: Option<&ToolRequest>,
    ) -> Self {
        let mut result = Self::new(code, message);
        if let Some(request) = request {
            result = result.with_tool_request(request);
            if request.is_write() {
                result.effect_outcome = Some("unknown");
            }
        }
        result
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
    #[cfg(feature = "mtproto")]
    MtprotoSuccess(crate::mtproto::ExecutionResult),
    #[cfg(feature = "mtproto")]
    MtprotoDryRun(crate::mtproto::DryRun),
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
            #[cfg(feature = "mtproto")]
            Self::MtprotoSuccess(value) => map.serialize_entry("result", value)?,
            #[cfg(feature = "mtproto")]
            Self::MtprotoDryRun(value) => map.serialize_entry("dry_run", value)?,
        }
        map.end()
    }
}
macro_rules! redacted {($($type:ident),+)=>{$(impl fmt::Debug for $type {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str(concat!(stringify!($type),"([REDACTED])"))}})+};}
redacted!(ToolSession, ToolError, ToolResponse);
#[cfg(feature = "mtproto")]
fn protocol_unavailable() -> ToolError {
    ToolError::new(
        "agent_protocol_unavailable",
        "This host session has no MTProto provider.",
    )
}
