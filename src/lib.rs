//! Predictable Telegram Bot API requests. No request is automatically retried.
//! On an ambiguous failure, inspect Error::outcome before considering another attempt.

mod client;
mod error;
mod requests;
mod types;

pub use client::{Bot, BotBuilder};
pub use error::{EffectOutcome, Error, ErrorKind, Result};
pub use requests::{RequestFuture, SendMessage};
pub use types::{Chat, ChatId, Message, MessageId, UserId};
