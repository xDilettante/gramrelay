use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fmt, io::Write, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JournalErrorKind {
    Validation,
    Capacity,
    Storage,
    Conflict,
    Denied,
    LeaseLost,
    Expired,
    UnsupportedSchema,
    Closed,
    CursorExpired,
}
/// Safe storage diagnostics. They do not determine whether a prior Telegram action succeeded.
#[derive(Debug)]
pub struct JournalError {
    kind: JournalErrorKind,
}
impl JournalError {
    pub(crate) fn new(kind: JournalErrorKind) -> Self {
        Self { kind }
    }
    pub fn kind(&self) -> JournalErrorKind {
        self.kind
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            JournalErrorKind::Validation => "journal_validation",
            JournalErrorKind::Capacity => "journal_capacity",
            JournalErrorKind::Storage => "journal_storage",
            JournalErrorKind::Conflict => "journal_conflict",
            JournalErrorKind::Denied => "journal_denied",
            JournalErrorKind::LeaseLost => "journal_lease_lost",
            JournalErrorKind::Expired => "journal_expired",
            JournalErrorKind::UnsupportedSchema => "journal_schema",
            JournalErrorKind::Closed => "journal_closed",
            JournalErrorKind::CursorExpired => "updates_cursor_expired",
        }
    }
}
impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for JournalError {}
pub type Result<T> = std::result::Result<T, JournalError>;

#[derive(Debug, Clone)]
pub struct JournalOptions {
    pub(super) max_operations: usize,
    pub(super) max_pending: usize,
    pub(super) lease: Duration,
    pub(super) retention: Duration,
    pub(super) max_result_bytes: usize,
    pub(super) retain_results: bool,
    pub(crate) max_update_entries: usize,
    pub(crate) max_update_bytes: usize,
    pub(crate) max_update_feeds: usize,
    pub(crate) max_update_consumers: usize,
}
impl Default for JournalOptions {
    fn default() -> Self {
        Self {
            max_operations: 100_000,
            max_pending: 16,
            lease: Duration::from_secs(60),
            retention: Duration::from_secs(7 * 24 * 3600),
            max_result_bytes: 65536,
            retain_results: true,
            max_update_entries: 100_000,
            max_update_bytes: 64 * 1024 * 1024,
            max_update_feeds: 64,
            max_update_consumers: 256,
        }
    }
}
impl JournalOptions {
    pub fn max_update_consumers(mut self, value: usize) -> Self {
        self.max_update_consumers = value;
        self
    }
    pub fn max_update_entries(mut self, value: usize) -> Self {
        self.max_update_entries = value;
        self
    }
    pub fn max_update_bytes(mut self, value: usize) -> Self {
        self.max_update_bytes = value;
        self
    }
    pub fn max_update_feeds(mut self, value: usize) -> Self {
        self.max_update_feeds = value;
        self
    }
    pub fn max_operations(mut self, value: usize) -> Self {
        self.max_operations = value;
        self
    }
    pub fn max_pending(mut self, value: usize) -> Self {
        self.max_pending = value;
        self
    }
    pub fn lease_duration(mut self, value: Duration) -> Self {
        self.lease = value;
        self
    }
    pub fn retention(mut self, value: Duration) -> Self {
        self.retention = value;
        self
    }
    pub fn max_result_bytes(mut self, value: usize) -> Self {
        self.max_result_bytes = value;
        self
    }
    pub fn retain_results(mut self, value: bool) -> Self {
        self.retain_results = value;
        self
    }
    pub(super) fn validate(&self) -> Result<()> {
        let bound = (self.max_result_bytes as u128 + 1024).checked_mul(self.max_operations as u128);
        let update_bound = (self.max_update_entries as u128)
            .checked_mul(1024)
            .and_then(|value| value.checked_add(self.max_update_bytes as u128));
        if self.max_operations == 0
            || self.max_operations > i64::MAX as usize
            || self.max_pending == 0
            || self.max_pending > 1024
            || self.max_result_bytes == 0
            || bound.is_none_or(|value| value > i64::MAX as u128)
            || self.max_update_entries == 0
            || self.max_update_bytes == 0
            || self.max_update_feeds == 0
            || self.max_update_feeds > 1024
            || self.max_update_consumers == 0
            || self.max_update_consumers > 1_000_000
            || update_bound.is_none_or(|value| value > i64::MAX as u128)
        {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        millis(self.lease)?;
        millis(self.retention)?;
        Ok(())
    }
}
pub(crate) fn millis(value: Duration) -> Result<i64> {
    i64::try_from(value.as_millis())
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| JournalError::new(JournalErrorKind::Validation))
}
pub(crate) fn valid_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(JournalError::new(JournalErrorKind::Validation))
    } else {
        Ok(())
    }
}

/// Immutable intent; only its fingerprint and bounded identifiers are persisted.
#[derive(Clone)]
pub struct OperationRequest {
    pub(super) profile: String,
    pub(super) principal: String,
    pub(super) id: String,
    pub(super) action: String,
    pub(super) hash: [u8; 32],
}
impl OperationRequest {
    pub fn new(
        profile: impl Into<String>,
        principal: impl Into<String>,
        id: impl Into<String>,
        action: impl Into<String>,
        payload: &Value,
    ) -> Result<Self> {
        let (profile, principal, id, action) =
            (profile.into(), principal.into(), id.into(), action.into());
        for value in [&profile, &principal, &id, &action] {
            valid_id(value)?;
        }
        let bytes = encode(payload, 65536)?;
        Ok(Self {
            profile,
            principal,
            id,
            action,
            hash: Sha256::digest(&bytes).into(),
        })
    }
}
pub(super) fn encode(value: &Value, limit: usize) -> Result<Vec<u8>> {
    let mut stack = vec![(value, 0usize)];
    let mut seen = 0usize;
    while let Some((value, depth)) = stack.pop() {
        seen += 1;
        if depth > 64 || seen > 8192 {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        let children = match value {
            Value::Array(values) => values.len(),
            Value::Object(values) => values.len(),
            _ => 0,
        };
        if children > 8192 - seen - stack.len() {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    canonical_json(value, limit)
}
pub(crate) fn canonical_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value)
        .map_err(|_| JournalError::new(JournalErrorKind::Validation))?;
    let mut canonical: Value = serde_json::from_slice(&output.bytes)
        .map_err(|_| JournalError::new(JournalErrorKind::Validation))?;
    canonical.sort_all_objects();
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| JournalError::new(JournalErrorKind::Validation))?;
    if bytes.len() > limit {
        return Err(JournalError::new(JournalErrorKind::Validation));
    }
    Ok(bytes)
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(std::io::Error::other("json_limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Prepared,
    InFlight,
    Succeeded,
    Rejected,
    Unknown,
}
impl OperationState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::InFlight => "in_flight",
            Self::Succeeded => "succeeded",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }
    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "in_flight" => Ok(Self::InFlight),
            "succeeded" => Ok(Self::Succeeded),
            "rejected" => Ok(Self::Rejected),
            "unknown" => Ok(Self::Unknown),
            _ => Err(JournalError::new(JournalErrorKind::Storage)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultAvailability {
    NotApplicable,
    Stored,
    Disabled,
    TooLarge,
    Expired,
}
impl ResultAvailability {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Stored => "stored",
            Self::Disabled => "disabled",
            Self::TooLarge => "too_large",
            Self::Expired => "expired",
        }
    }
    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "not_applicable" => Ok(Self::NotApplicable),
            "stored" => Ok(Self::Stored),
            "disabled" => Ok(Self::Disabled),
            "too_large" => Ok(Self::TooLarge),
            "expired" => Ok(Self::Expired),
            _ => Err(JournalError::new(JournalErrorKind::Storage)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptFailure {
    pub(super) kind: String,
    pub(super) outcome: String,
    pub(super) http_status: Option<u16>,
    pub(super) api_error_code: Option<i64>,
    #[serde(default)]
    pub(super) retry_after_secs: Option<u64>,
    #[serde(default)]
    pub(super) migrate_to_chat_id: Option<i64>,
}
impl AttemptFailure {
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn outcome(&self) -> &str {
        &self.outcome
    }
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after_secs.map(Duration::from_secs)
    }
    pub fn migrate_to_chat_id(&self) -> Option<tg::ChatId> {
        self.migrate_to_chat_id.map(tg::ChatId::from)
    }
}
/// A known result or a classified library failure, without remote descriptions.
pub struct Completion {
    pub(super) state: OperationState,
    pub(super) result: Option<Value>,
    pub(super) failure: Option<AttemptFailure>,
}
impl Completion {
    pub fn succeeded(result: Value) -> Self {
        Self {
            state: OperationState::Succeeded,
            result: Some(result),
            failure: None,
        }
    }
    pub fn from_error(error: &tg::Error) -> Self {
        let (state, outcome) = match error.outcome() {
            tg::EffectOutcome::NotSent => (OperationState::Prepared, "not_sent"),
            tg::EffectOutcome::Rejected => (OperationState::Rejected, "rejected"),
            _ => (OperationState::Unknown, "unknown"),
        };
        Self {
            state,
            result: None,
            failure: Some(AttemptFailure {
                kind: match error.kind() {
                    tg::ErrorKind::Configuration => "configuration",
                    tg::ErrorKind::Validation => "validation",
                    tg::ErrorKind::Capacity => "capacity",
                    tg::ErrorKind::Io => "io",
                    tg::ErrorKind::Timeout => "timeout",
                    tg::ErrorKind::Transport => "transport",
                    tg::ErrorKind::Http => "http",
                    tg::ErrorKind::Telegram => "telegram",
                    tg::ErrorKind::Decode => "decode",
                    tg::ErrorKind::ResponseTooLarge => "response_too_large",
                    tg::ErrorKind::Handler => "handler",
                    _ => "other",
                }
                .into(),
                outcome: outcome.into(),
                http_status: error.http_status(),
                api_error_code: error.api_error_code(),
                retry_after_secs: error.retry_after().map(|duration| duration.as_secs()),
                migrate_to_chat_id: error.migrate_to_chat_id().map(i64::from),
            }),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Operation {
    pub(super) profile: String,
    pub(super) operation_id: String,
    pub(super) action: String,
    pub(super) state: OperationState,
    pub(super) revision: i64,
    pub(super) created_at_ms: i64,
    pub(super) updated_at_ms: i64,
    pub(super) expires_at_ms: i64,
    pub(super) expired: bool,
    pub(super) result_availability: ResultAvailability,
    pub(super) result: Option<Value>,
    pub(super) failure: Option<AttemptFailure>,
}
impl Operation {
    pub fn state(&self) -> OperationState {
        self.state
    }
    pub fn revision(&self) -> i64 {
        self.revision
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn action(&self) -> &str {
        &self.action
    }
    pub fn result(&self) -> Option<&Value> {
        self.result.as_ref()
    }
    pub fn result_availability(&self) -> ResultAvailability {
        self.result_availability
    }
    pub fn failure(&self) -> Option<&AttemptFailure> {
        self.failure.as_ref()
    }
    pub fn expired(&self) -> bool {
        self.expired
    }
}
#[derive(Clone)]
pub(super) struct Lease {
    pub issuer: String,
    pub request: OperationRequest,
    pub revision: i64,
}
#[derive(Clone)]
pub struct Prepared(pub(super) Lease);
#[derive(Clone)]
pub struct InFlight(pub(super) Lease);
#[derive(Debug)]
pub enum Preparation {
    Ready(Prepared),
    Existing(Operation),
}

macro_rules! redacted_debug {
    ($($name:ty),* $(,)?) => {$ (impl fmt::Debug for $name { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct(stringify!($name)).finish_non_exhaustive() } })*};
}
redacted_debug!(OperationRequest, Prepared, InFlight, Completion);
impl fmt::Debug for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Operation")
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("result_availability", &self.result_availability)
            .finish_non_exhaustive()
    }
}
