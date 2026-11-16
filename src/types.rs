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

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Chat {
    pub id: ChatId,
    #[serde(rename = "type")]
    pub kind: String,
}

/// The supported subset of a Telegram message. New response fields are tolerated.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Message {
    #[serde(rename = "message_id")]
    pub id: MessageId,
    pub date: i64,
    pub chat: Chat,
    pub text: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct User {
    pub id: UserId,
    pub is_bot: bool,
    pub first_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct CallbackQuery {
    pub id: CallbackQueryId,
    pub from: User,
    pub chat_instance: String,
    pub data: Option<String>,
    pub message: Option<Message>,
    pub inline_message_id: Option<String>,
}

/// Decode an update received by the host. This type does not start a receiver.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
    /// Preserve update fields not yet modeled by this prototype.
    #[serde(flatten)]
    pub unknown_fields: std::collections::BTreeMap<String, serde_json::Value>,
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
