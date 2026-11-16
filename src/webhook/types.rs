use crate::{Error, ErrorKind, Result, Update};
use std::{fmt, future::Future, pin::Pin, time::Duration};
use subtle::ConstantTimeEq;

/// A distinct incoming secret, not the bot token. Explicit exposure is needed for setWebhook.
#[derive(Clone)]
pub struct WebhookSecret(String);
impl WebhookSecret {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if !(1..=256).contains(&value.len())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(Error::local(ErrorKind::Validation, "webhook_secret"));
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub(super) fn matches(&self, supplied: Option<&[u8]>) -> bool {
        supplied
            .is_some_and(|value| value.len() <= 256 && bool::from(self.0.as_bytes().ct_eq(value)))
    }
}
impl fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebhookSecret([REDACTED])")
    }
}

/// Fixed diagnostics suitable for an untrusted HTTP peer. No private cause is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    BadRequest,
    Unauthorized,
    PayloadTooLarge,
    UnsupportedMediaType,
    Conflict,
    Busy,
    Closed,
    Unavailable,
    Timeout,
    Handler,
}
impl WebhookError {
    pub fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::PayloadTooLarge => 413,
            Self::UnsupportedMediaType => 415,
            Self::Conflict => 409,
            Self::Busy | Self::Closed | Self::Unavailable => 503,
            Self::Timeout => 504,
            Self::Handler => 500,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::BadRequest => "webhook_bad_request",
            Self::Unauthorized => "webhook_unauthorized",
            Self::PayloadTooLarge => "webhook_payload_too_large",
            Self::UnsupportedMediaType => "webhook_media_type",
            Self::Conflict => "webhook_conflict",
            Self::Busy => "webhook_busy",
            Self::Closed => "webhook_closed",
            Self::Unavailable => "webhook_unavailable",
            Self::Timeout => "webhook_timeout",
            Self::Handler => "webhook_handler",
        }
    }
}
impl fmt::Display for WebhookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for WebhookError {}
pub type WebhookFuture<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<(), WebhookError>> + Send + 'a>>;

/// Trusted host integration. Success must mean processed or durably committed, never just queued.
/// The host coordinates sources, identity, deduplication and any separately owned worker tasks.
pub trait WebhookSink: Send + Sync + 'static {
    /// Any backend-owned queue/worker must retain this original admission deadline.
    fn ingest(&self, update: Update, deadline: tokio::time::Instant) -> WebhookFuture<'_>;
}

#[derive(Clone)]
pub struct WebhookOptions {
    pub(super) path: String,
    pub(super) max_body_bytes: usize,
    pub(super) max_buffered_bytes: usize,
    pub(super) max_in_flight: usize,
    pub(super) request_timeout: Duration,
    pub(super) shutdown_timeout: Duration,
    pub(super) max_connections: usize,
    pub(super) header_timeout: Duration,
    pub(super) connection_timeout: Duration,
    pub(super) trusted_proxy: bool,
}
impl Default for WebhookOptions {
    fn default() -> Self {
        Self {
            path: "/telegram".into(),
            max_body_bytes: 1024 * 1024,
            max_buffered_bytes: 16 * 1024 * 1024,
            max_in_flight: 16,
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            max_connections: 64,
            header_timeout: Duration::from_secs(5),
            connection_timeout: Duration::from_secs(60),
            trusted_proxy: false,
        }
    }
}
impl fmt::Debug for WebhookOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookOptions")
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_buffered_bytes", &self.max_buffered_bytes)
            .field("max_in_flight", &self.max_in_flight)
            .finish_non_exhaustive()
    }
}
impl WebhookOptions {
    pub fn path(mut self, value: impl Into<String>) -> Self {
        self.path = value.into();
        self
    }
    pub fn max_body_bytes(mut self, value: usize) -> Self {
        self.max_body_bytes = value;
        self
    }
    pub fn max_buffered_bytes(mut self, value: usize) -> Self {
        self.max_buffered_bytes = value;
        self
    }
    pub fn max_in_flight(mut self, value: usize) -> Self {
        self.max_in_flight = value;
        self
    }
    pub fn request_timeout(mut self, value: Duration) -> Self {
        self.request_timeout = value;
        self
    }
    pub fn shutdown_timeout(mut self, value: Duration) -> Self {
        self.shutdown_timeout = value;
        self
    }
    pub fn max_connections(mut self, value: usize) -> Self {
        self.max_connections = value;
        self
    }
    pub fn header_timeout(mut self, value: Duration) -> Self {
        self.header_timeout = value;
        self
    }
    pub fn connection_timeout(mut self, value: Duration) -> Self {
        self.connection_timeout = value;
        self
    }
    /// Permit a plaintext non-loopback listener behind the host's trusted TLS proxy.
    pub fn trusted_proxy(mut self, value: bool) -> Self {
        self.trusted_proxy = value;
        self
    }
    pub fn validate(&self) -> Result<()> {
        if !self.path.starts_with('/')
            || self.path.len() > 256
            || !self
                .path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-'))
            || !(1..=16 * 1024 * 1024).contains(&self.max_body_bytes)
            || !(1..=1024).contains(&self.max_in_flight)
            || self.max_buffered_bytes < self.max_body_bytes
            || self.max_buffered_bytes > 1024 * 1024 * 1024
            || !(1..=1024).contains(&self.max_connections)
            || [
                self.request_timeout,
                self.shutdown_timeout,
                self.header_timeout,
                self.connection_timeout,
            ]
            .into_iter()
            .any(|d| d.is_zero() || tokio::time::Instant::now().checked_add(d).is_none())
        {
            return Err(Error::local(ErrorKind::Validation, "webhook_options"));
        }
        Ok(())
    }
}
