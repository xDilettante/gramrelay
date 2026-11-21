pub use crate::api::types::{CallbackQuery, Chat, Document, Message, Update, User, WebhookInfo};
use crate::{
    ButtonStyle, Error, ErrorKind, Result,
    api::types::{
        CopyTextButton, InlineKeyboardButton as WireKeyboardButton,
        InlineKeyboardMarkup as WireKeyboardMarkup,
    },
};
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
    /// Explicit access for typed adapters; an identifier is not an access grant.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Inline button constructors backed by the generated Bot API type.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct InlineKeyboardButton(WireKeyboardButton);

impl InlineKeyboardButton {
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Self(WireKeyboardButton::new(text.into()).callback_data(data.into()))
    }

    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self(WireKeyboardButton::new(text.into()).url(url.into()))
    }

    pub fn copy_text(text: impl Into<String>, value: impl Into<String>) -> Self {
        Self(WireKeyboardButton::new(text.into()).copy_text(CopyTextButton::new(value.into())))
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.0.style = style.as_str().map(str::to_owned);
        self
    }
}

impl From<WireKeyboardButton> for InlineKeyboardButton {
    fn from(button: WireKeyboardButton) -> Self {
        Self(button)
    }
}

impl From<InlineKeyboardButton> for WireKeyboardButton {
    fn from(button: InlineKeyboardButton) -> Self {
        button.0
    }
}

/// Inline keyboard builder that can also wrap a generated markup value.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct InlineKeyboardMarkup(WireKeyboardMarkup);

impl Default for InlineKeyboardMarkup {
    fn default() -> Self {
        Self(WireKeyboardMarkup::new(Vec::new()))
    }
}

impl InlineKeyboardMarkup {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn row(mut self, buttons: impl IntoIterator<Item = InlineKeyboardButton>) -> Self {
        self.0
            .inline_keyboard
            .push(buttons.into_iter().map(Into::into).collect());
        self
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for button in self.0.inline_keyboard.iter().flatten() {
            if button.text.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "button_text"));
            }
            if button
                .callback_data
                .as_ref()
                .is_some_and(|data| !(1..=64).contains(&data.len()))
            {
                return Err(Error::local(ErrorKind::Validation, "callback_data"));
            }
            if button.url.as_ref().is_some_and(|url| {
                !reqwest::Url::parse(url).is_ok_and(|url| {
                    url.has_host() && matches!(url.scheme(), "http" | "https" | "tg")
                })
            }) {
                return Err(Error::local(ErrorKind::Validation, "button_url"));
            }
            if button
                .copy_text
                .as_ref()
                .is_some_and(|copy| !(1..=256).contains(&copy.text.chars().count()))
            {
                return Err(Error::local(ErrorKind::Validation, "copy_text"));
            }
            let actions = [
                button.url.is_some(),
                button.callback_data.is_some(),
                button.web_app.is_some(),
                button.login_url.is_some(),
                button.switch_inline_query.is_some(),
                button.switch_inline_query_current_chat.is_some(),
                button.switch_inline_query_chosen_chat.is_some(),
                button.copy_text.is_some(),
                button.callback_game.is_some(),
                button.pay.is_some(),
                button.disabled.is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count();
            // Preserve forward compatibility when the generated type carries an unknown action.
            if actions > 1 || (actions == 0 && button.unknown_fields().is_empty()) {
                return Err(Error::local(ErrorKind::Validation, "button_action"));
            }
        }
        Ok(())
    }
}

impl From<WireKeyboardMarkup> for InlineKeyboardMarkup {
    fn from(markup: WireKeyboardMarkup) -> Self {
        Self(markup)
    }
}

impl From<InlineKeyboardMarkup> for WireKeyboardMarkup {
    fn from(markup: InlineKeyboardMarkup) -> Self {
        markup.0
    }
}
