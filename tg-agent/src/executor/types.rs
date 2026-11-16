use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};
use std::fmt;

pub(super) type Result<T> = std::result::Result<T, ExecutorError>;

/// Diagnostics contain static categories only, never tool input or remote errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecutorErrorKind {
    InvalidInput,
    InvalidPolicy,
    Denied,
    ApprovalRequired,
    ContextRequired,
    Capacity,
    Conflict,
    QuotaExceeded,
    Storage,
    Identity,
    UnsupportedAction,
    Uncertain,
    InvalidResponse,
    NotFound,
    Remote,
    CursorExpired,
}
#[derive(Debug)]
pub struct ExecutorError {
    kind: ExecutorErrorKind,
    outcome: Option<tg::EffectOutcome>,
}
impl ExecutorError {
    pub(super) fn new(kind: ExecutorErrorKind) -> Self {
        Self {
            kind,
            outcome: None,
        }
    }
    pub fn kind(&self) -> ExecutorErrorKind {
        self.kind
    }
    pub fn outcome(&self) -> Option<tg::EffectOutcome> {
        self.outcome
    }
    pub(super) fn with_outcome(mut self, outcome: tg::EffectOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            ExecutorErrorKind::InvalidInput => "executor_invalid_input",
            ExecutorErrorKind::InvalidPolicy => "executor_invalid_policy",
            ExecutorErrorKind::Denied => "executor_denied",
            ExecutorErrorKind::ApprovalRequired => "executor_approval_required",
            ExecutorErrorKind::ContextRequired => "executor_context_required",
            ExecutorErrorKind::Capacity => "executor_capacity",
            ExecutorErrorKind::Conflict => "executor_conflict",
            ExecutorErrorKind::QuotaExceeded => "executor_quota_exceeded",
            ExecutorErrorKind::Storage => "executor_storage",
            ExecutorErrorKind::Identity => "executor_identity",
            ExecutorErrorKind::UnsupportedAction => "executor_unsupported_action",
            ExecutorErrorKind::Uncertain => "executor_uncertain",
            ExecutorErrorKind::InvalidResponse => "executor_invalid_response",
            ExecutorErrorKind::NotFound => "executor_not_found",
            ExecutorErrorKind::Remote => "executor_remote",
            ExecutorErrorKind::CursorExpired => "executor_cursor_expired",
        }
    }
}
impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for ExecutorError {}

/// Version 1 tools, in stable discovery order. There is no dynamic method escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    GetMe,
    GetChat,
    ReadUpdates,
    SendMessage,
    EditMessageText,
    AnswerCallbackQuery,
    GetOperation,
    SendDocument,
}
impl Action {
    pub const ALL: [Self; 8] = [
        Self::GetMe,
        Self::GetChat,
        Self::ReadUpdates,
        Self::SendMessage,
        Self::EditMessageText,
        Self::AnswerCallbackQuery,
        Self::GetOperation,
        Self::SendDocument,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::GetMe => "get_me",
            Self::GetChat => "get_chat",
            Self::ReadUpdates => "read_updates",
            Self::SendMessage => "send_message",
            Self::EditMessageText => "edit_message_text",
            Self::AnswerCallbackQuery => "answer_callback_query",
            Self::GetOperation => "get_operation",
            Self::SendDocument => "send_document",
        }
    }
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::SendMessage
                | Self::EditMessageText
                | Self::AnswerCallbackQuery
                | Self::SendDocument
        )
    }
}

/// Canonical nonzero signed decimal string on the wire; usernames are not scopes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChatId(i64);
impl ChatId {
    pub fn new(value: i64) -> Result<Self> {
        if value == 0 {
            return Err(invalid());
        }
        Ok(Self(value))
    }
    pub fn get(self) -> i64 {
        self.0
    }
}
/// Positive decimal string, including in JSON/JavaScript clients.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MessageId(i64);
impl MessageId {
    pub fn new(value: i64) -> Result<Self> {
        if value <= 0 {
            return Err(invalid());
        }
        Ok(Self(value))
    }
    pub fn get(self) -> i64 {
        self.0
    }
}
fn numeric_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<i64, D::Error> {
    let value =
        String::deserialize(deserializer).map_err(|_| D::Error::custom("invalid numeric ID"))?;
    let number: i64 = value
        .parse()
        .map_err(|_| D::Error::custom("invalid numeric ID"))?;
    if number.to_string() != value {
        return Err(D::Error::custom("invalid numeric ID"));
    }
    Ok(number)
}
macro_rules! numeric_id {
    ($name:ident) => {
        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                Self::new(numeric_string(deserializer)?)
                    .map_err(|_| D::Error::custom("invalid numeric ID"))
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}
numeric_id!(ChatId);
numeric_id!(MessageId);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetChatArgs {
    pub chat_id: ChatId,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadUpdatesArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u8,
}
fn default_limit() -> u8 {
    20
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageArgs {
    pub chat_id: ChatId,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<MessageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditMessageTextArgs {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerCallbackQueryArgs {
    pub callback_query_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_alert: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetOperationArgs {
    pub operation_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendDocumentArgs {
    pub chat_id: ChatId,
    pub file_handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Explicit argument variants keep injected transport/policy fields out of tools.
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "tool",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ActionCall {
    GetMe(#[serde(deserialize_with = "map_only")] EmptyArgs),
    GetChat(#[serde(deserialize_with = "map_only")] GetChatArgs),
    ReadUpdates(#[serde(deserialize_with = "map_only")] ReadUpdatesArgs),
    SendMessage(#[serde(deserialize_with = "map_only")] SendMessageArgs),
    EditMessageText(#[serde(deserialize_with = "map_only")] EditMessageTextArgs),
    AnswerCallbackQuery(#[serde(deserialize_with = "map_only")] AnswerCallbackQueryArgs),
    GetOperation(#[serde(deserialize_with = "map_only")] GetOperationArgs),
    SendDocument(#[serde(deserialize_with = "map_only")] SendDocumentArgs),
}
impl ActionCall {
    pub fn action(&self) -> Action {
        match self {
            Self::GetMe(_) => Action::GetMe,
            Self::GetChat(_) => Action::GetChat,
            Self::ReadUpdates(_) => Action::ReadUpdates,
            Self::SendMessage(_) => Action::SendMessage,
            Self::EditMessageText(_) => Action::EditMessageText,
            Self::AnswerCallbackQuery(_) => Action::AnswerCallbackQuery,
            Self::GetOperation(_) => Action::GetOperation,
            Self::SendDocument(_) => Action::SendDocument,
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::GetMe(_) | Self::GetChat(_) => {}
            Self::ReadUpdates(args) => {
                if args.limit == 0
                    || args.limit > 100
                    || args
                        .cursor
                        .as_ref()
                        .is_some_and(|v| v.is_empty() || v.len() > 1024)
                {
                    return Err(invalid());
                }
            }
            Self::SendMessage(args) => text(&args.text, 1, 4096)?,
            Self::EditMessageText(args) => text(&args.text, 1, 4096)?,
            Self::AnswerCallbackQuery(args) => {
                if args.callback_query_id.is_empty()
                    || args.callback_query_id.len() > 256
                    || args.callback_query_id.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                if let Some(value) = &args.text {
                    text(value, 0, 200)?;
                }
            }
            Self::GetOperation(args) => valid_id(&args.operation_id)?,
            Self::SendDocument(args) => {
                valid_id(&args.file_handle)?;
                if let Some(value) = &args.caption {
                    text(value, 0, 1024)?;
                }
            }
        }
        Ok(())
    }
}
fn text(value: &str, min: usize, max: usize) -> Result<()> {
    let length = value.chars().count();
    if length < min || length > max || value.contains('\0') {
        Err(invalid())
    } else {
        Ok(())
    }
}
pub(super) fn valid_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
    {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn invalid() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidInput)
}

/// Immutable validated versioned request. Host authority is deliberately absent.
#[derive(Clone, Serialize)]
pub struct Request {
    schema_version: u32,
    profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    call: ActionCall,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    schema_version: u32,
    profile: String,
    #[serde(default, deserialize_with = "present_operation_id")]
    operation_id: Option<String>,
    #[serde(deserialize_with = "map_only")]
    call: ActionCall,
}
// Serde's derived struct/adjacent-enum visitors also accept positional arrays.
// Restrict the tool boundary to objects while retaining duplicate-field checks.
fn map_only<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            map: M,
        ) -> std::result::Result<T, M::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Object(std::marker::PhantomData))
}
fn present_operation_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    // Absent is allowed for reads; an explicitly present null is never an ID.
    String::deserialize(deserializer).map(Some)
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire: WireRequest =
            map_only(deserializer).map_err(|_| D::Error::custom("invalid tool request"))?;
        if wire.schema_version != 1 {
            return Err(D::Error::custom("invalid tool request"));
        }
        Self::new(wire.profile, wire.operation_id, wire.call)
            .map_err(|_| D::Error::custom("invalid tool request"))
    }
}
impl Request {
    pub const MAX_JSON_BYTES: usize = 65536;
    /// Bounded JSON entry point for untrusted adapters; parser details are redacted.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > Self::MAX_JSON_BYTES {
            return Err(invalid());
        }
        serde_json::from_slice(bytes).map_err(|_| invalid())
    }
    pub fn new(
        profile: impl Into<String>,
        operation_id: Option<String>,
        call: ActionCall,
    ) -> Result<Self> {
        let value = Self {
            schema_version: 1,
            profile: profile.into(),
            operation_id,
            call,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn operation_id(&self) -> Option<&str> {
        self.operation_id.as_deref()
    }
    pub fn call(&self) -> &ActionCall {
        &self.call
    }
    pub fn action(&self) -> Action {
        self.call.action()
    }
    fn validate(&self) -> Result<()> {
        valid_id(&self.profile)?;
        if self.action().is_write() != self.operation_id.is_some() {
            return Err(invalid());
        }
        if let Some(id) = &self.operation_id {
            valid_id(id)?;
        }
        self.call.validate()?;
        // The typed tree has fixed depth/nodes. This also bounds Rust-constructed inputs.
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > Self::MAX_JSON_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
    pub(super) fn fingerprint(&self) -> [u8; 32] {
        // Fixed struct/variant ordering, validated immutable fields and normalized defaults.
        // Includes operation_id so one approval cannot authorize another write key.
        Sha256::digest(serde_json::to_vec(self).expect("validated request serialization")).into()
    }
}
macro_rules! redacted_debug {
    ($($name:ident),+ $(,)?) => { $(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    Request,
    ActionCall,
    EmptyArgs,
    GetChatArgs,
    ReadUpdatesArgs,
    SendMessageArgs,
    EditMessageTextArgs,
    AnswerCallbackQueryArgs,
    GetOperationArgs,
    SendDocumentArgs
);
