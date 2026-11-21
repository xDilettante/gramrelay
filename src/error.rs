use crate::ChatId;
use std::{fmt, time::Duration};

/// The reason a request could not produce a confirmed result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    Configuration,
    Validation,
    Capacity,
    Io,
    Timeout,
    Transport,
    Http,
    Telegram,
    Decode,
    ResponseTooLarge,
    Handler,
}

/// What is known about the remote effect of a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectOutcome {
    /// The library did not dispatch the request.
    NotSent,
    /// A valid Bot API response explicitly refused the request.
    Rejected,
    /// Telegram may have applied the action. Do not blindly retry it.
    Unknown,
}

/// A safe diagnostic. Raw HTTP errors, URLs and remote descriptions are omitted from
/// `Display`, `Debug` and error chains.
pub struct Error {
    kind: ErrorKind,
    outcome: EffectOutcome,
    field: Option<&'static str>,
    pub(crate) http_status: Option<u16>,
    pub(crate) api_error_code: Option<i64>,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) migrate_to_chat_id: Option<ChatId>,
    pub(crate) remote_description: Option<String>,
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately omit remote_description so Debug remains safe for logs and receipts.
        f.debug_struct("Error")
            .field("kind", &self.kind)
            .field("outcome", &self.outcome)
            .field("field", &self.field)
            .field("http_status", &self.http_status)
            .field("api_error_code", &self.api_error_code)
            .field("retry_after", &self.retry_after)
            .field("migrate_to_chat_id", &self.migrate_to_chat_id)
            .finish()
    }
}

impl Error {
    /// Report an application-handler failure without retaining sensitive underlying errors.
    /// The handler may already have performed side effects, so the outcome is Unknown.
    pub fn handler(operation: &'static str) -> Self {
        Self {
            field: Some(operation),
            ..Self::new(ErrorKind::Handler, EffectOutcome::Unknown)
        }
    }

    pub(crate) fn new(kind: ErrorKind, outcome: EffectOutcome) -> Self {
        Self {
            kind,
            outcome,
            field: None,
            http_status: None,
            api_error_code: None,
            retry_after: None,
            migrate_to_chat_id: None,
            remote_description: None,
        }
    }

    pub(crate) fn local(kind: ErrorKind, field: &'static str) -> Self {
        Self {
            field: Some(field),
            ..Self::new(kind, EffectOutcome::NotSent)
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
    pub fn outcome(&self) -> EffectOutcome {
        self.outcome
    }
    /// The invalid field name, without its supplied value.
    pub fn field(&self) -> Option<&'static str> {
        self.field
    }
    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }
    pub fn api_error_code(&self) -> Option<i64> {
        self.api_error_code
    }
    /// Server backoff advice, honored by an explicitly enabled RetryPolicy.
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
    /// The caller must authorize a new target before trying it.
    pub fn migrate_to_chat_id(&self) -> Option<ChatId> {
        self.migrate_to_chat_id
    }
    /// Bounded, sanitized copy of Telegram's remote error `description`, retained only when
    /// explicitly opted in via `BotBuilder::retain_remote_descriptions(true)`.
    ///
    /// Still untrusted remote text that may reflect user data or request parameters; never
    /// emitted by `Display`, `Debug`, telemetry, or agent receipts.
    pub fn untrusted_remote_description(&self) -> Option<&str> {
        self.remote_description.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: effect={:?}", self.kind, self.outcome)?;
        if let Some(field) = self.field {
            write!(f, ", field={field}")?;
        }
        if let Some(code) = self.api_error_code {
            write!(f, ", telegram_code={code}")?;
        }
        Ok(())
    }
}

// Deliberately no raw reqwest/io source: those can contain tokens or private paths.
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
