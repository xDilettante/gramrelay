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
