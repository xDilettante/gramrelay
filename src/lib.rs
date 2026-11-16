//! Predictable Telegram Bot API requests. No request is automatically retried.
//! On an ambiguous failure, inspect Error::outcome before considering another attempt.
//!
//! ```no_run
//! use tg::{Bot, ChatId};
//! # async fn example() -> tg::Result<()> {
//! let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
//! let sent = bot.send_message(ChatId::from(123456789), "Hello").await?;
//! println!("message_id={}", i64::from(sent.id));
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod files;
mod requests;
mod types;

pub use client::{Bot, BotBuilder};
pub use error::{EffectOutcome, Error, ErrorKind, Result};
pub use files::InputFile;
pub use requests::{AnswerCallbackQuery, RequestFuture, SendDocument, SendMessage};
pub use types::{
    CallbackQuery, CallbackQueryId, Chat, ChatId, Document, FileId, InlineKeyboardButton,
    InlineKeyboardMarkup, Message, MessageId, Update, User, UserId,
};
