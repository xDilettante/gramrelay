//! Durable storage and shared execution for Telegram agent tools.
//! Journal ownership is a host boundary, not a grant of Telegram permissions.

#[cfg(feature = "executor")]
pub mod executor;
pub mod journal;
#[cfg(feature = "bot")]
pub mod runtime;
pub mod updates;
#[cfg(feature = "webhook")]
pub mod webhook;
