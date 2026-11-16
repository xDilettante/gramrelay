pub use crate::api::types::{CallbackQuery, Chat, Document, Message, Update, User, WebhookInfo};
use serde::{Deserialize, Serialize};

/// A Telegram chat identifier, including negative group identifiers.
///
/// ```compile_fail
/// use tg::{Bot, UserId};
/// async fn example(bot: Bot, user: UserId) {
///     bot.send_message(user, "wrong ID type").await.unwrap();
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChatId(i64);
impl From<i64> for ChatId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}
impl From<ChatId> for i64 {
    fn from(value: ChatId) -> Self {
        value.0
    }
}

/// A message identifier scoped to its chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(i64);
impl From<i64> for MessageId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}
impl From<MessageId> for i64 {
    fn from(value: MessageId) -> Self {
        value.0
    }
}

/// A user identifier, intentionally not convertible to ChatId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(i64);
impl From<i64> for UserId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}
impl From<UserId> for i64 {
    fn from(value: UserId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(String);
impl From<String> for FileId {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<&str> for FileId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl FileId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque identifier supplied by Telegram for a callback query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallbackQueryId(String);
impl From<String> for CallbackQueryId {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<&str> for CallbackQueryId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl CallbackQueryId {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineKeyboardButton {
    text: String,
    callback_data: String,
}
impl InlineKeyboardButton {
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            callback_data: data.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct InlineKeyboardMarkup {
    inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}
impl InlineKeyboardMarkup {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn row(mut self, buttons: impl IntoIterator<Item = InlineKeyboardButton>) -> Self {
        self.inline_keyboard.push(buttons.into_iter().collect());
        self
    }
    pub(crate) fn validate(&self) -> crate::Result<()> {
        for button in self.inline_keyboard.iter().flatten() {
            if button.text.is_empty() {
                return Err(crate::Error::local(
                    crate::ErrorKind::Validation,
                    "button_text",
                ));
            }
            // https://core.telegram.org/bots/api#inlinekeyboardbutton : 1-64 bytes.
            if !(1..=64).contains(&button.callback_data.len()) {
                return Err(crate::Error::local(
                    crate::ErrorKind::Validation,
                    "callback_data",
                ));
            }
        }
        Ok(())
    }
}
