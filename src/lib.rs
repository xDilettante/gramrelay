//! Predictable Telegram Bot API requests. Retries are opt-in; unknown writes never replay.
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
#[cfg(feature = "bot")]
mod dispatch;
mod downloads;
mod error;
mod events;
mod files;
#[cfg(feature = "bot")]
mod polling;
mod requests;
mod retry;
pub mod rich_message;
#[cfg(feature = "bot")]
mod routing;
mod source;
mod telemetry;
#[cfg(feature = "test-kit")]
pub mod test_kit;
mod text;
mod typed_api;
mod types;
mod updates;
#[cfg(feature = "webhook")]
pub mod webhook;
mod wire;

#[path = "generated/mod.rs"]
pub mod api;

pub use client::{Bot, BotBuilder};
#[cfg(feature = "bot")]
pub use dispatch::DispatchOptions;
pub use downloads::Download;
pub use error::{EffectOutcome, Error, ErrorKind, Result};
pub use files::InputFile;
#[cfg(feature = "bot")]
pub use polling::{PollingOptions, PollingReconnect};
pub use requests::{
    AnswerCallbackQuery, RequestFuture, RichMessageInput, SendDocument, SendMessage,
    SendRichMessage,
};
pub use retry::RetryPolicy;
pub use rich_message::{ButtonStyle, RichDoc};
#[cfg(feature = "bot")]
pub use routing::{CallbackContext, MessageContext, MessageEvent, Router, UpdateContext};
pub use source::UpdateSourceGuard;
pub use text::{EntityKind, EntityText, ParseMode, RichText};
#[cfg(feature = "bot")]
pub use tokio_util::sync::CancellationToken;
pub use types::{
    CallbackQuery, CallbackQueryId, Chat, ChatId, Document, FileId, InlineKeyboardButton,
    InlineKeyboardMarkup, Message, MessageId, Update, User, UserId, WebhookInfo,
};
pub use updates::GetUpdates;
