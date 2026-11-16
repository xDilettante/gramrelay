//! Authenticated bounded intake. Low-level sinks own source coordination and acknowledgement.
mod http;
mod ingress;
mod server;
mod types;
pub use ingress::{WebhookIngress, WebhookRequest};
pub use types::{WebhookError, WebhookFuture, WebhookOptions, WebhookSecret, WebhookSink};
