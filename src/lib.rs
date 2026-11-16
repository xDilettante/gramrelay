//! Predictable Telegram Bot API requests. No request is automatically retried.
//! On an ambiguous failure, inspect Error::outcome before considering another attempt.
//!
//! ```no_run
//! use tg::{Bot, ChatId};
//! # async fn example() -> tg::Result<()> {
//! let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
//! let sent = bot.send_message(ChatId::from(123456789), "Hello").await?;
//! println!("message_id={}", i64::from(sent.message_id));
//! # Ok(())
//! # }
//! ```

mod client;
mod delivery;
mod error;
mod events;
mod files;
#[cfg(feature = "bot")]
mod polling;
mod requests;
#[cfg(feature = "bot")]
mod routing;
mod text;
mod typed_api;
mod types;
mod updates;
mod wire;

#[path = "generated/mod.rs"]
pub mod api;

pub use client::{Bot, BotBuilder};
pub use error::{EffectOutcome, Error, ErrorKind, Result};
pub use files::InputFile;
#[cfg(feature = "bot")]
pub use polling::PollingOptions;
pub use requests::{AnswerCallbackQuery, RequestFuture, SendDocument, SendMessage};
#[cfg(feature = "bot")]
pub use routing::{CallbackContext, MessageContext, MessageEvent, Router, UpdateContext};
pub use text::{EntityKind, ParseMode, RichText};
#[cfg(feature = "bot")]
pub use tokio_util::sync::CancellationToken;
pub use types::{
    CallbackQuery, CallbackQueryId, Chat, ChatId, Document, FileId, InlineKeyboardButton,
    InlineKeyboardMarkup, Message, MessageId, Update, User, UserId, WebhookInfo,
};
pub use updates::GetUpdates;
