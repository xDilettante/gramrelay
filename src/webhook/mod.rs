//! Authenticated bounded intake. Low-level sinks own source coordination and acknowledgement.
mod http;
mod ingress;
mod memory;
mod server;
mod telemetry;
mod types;
pub use ingress::{WebhookIngress, WebhookRequest};
pub use memory::{MemoryWebhook, MemoryWebhookOptions};
pub use types::{WebhookError, WebhookFuture, WebhookOptions, WebhookSecret, WebhookSink};
