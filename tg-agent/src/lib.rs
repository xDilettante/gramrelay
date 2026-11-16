//! Durable storage and shared execution for Telegram agent tools.
//! Journal ownership is a host boundary, not a grant of Telegram permissions.

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "executor")]
pub mod executor;
#[cfg(feature = "host")]
pub mod host;
pub mod journal;
#[cfg(feature = "bot")]
pub mod runtime;
#[cfg(feature = "executor")]
pub mod tools;
pub mod updates;
#[cfg(feature = "webhook")]
pub mod webhook;
