//! Durable storage and shared execution for Telegram agent tools.
//! Journal ownership is a host boundary, not a grant of Telegram permissions.

pub mod journal;
pub mod updates;
