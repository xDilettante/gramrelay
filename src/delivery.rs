use crate::{
    CallbackQueryId, ChatId, Error, ErrorKind, Result,
    api::types::{EphemeralMessageParameters, ReplyParameters},
};
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Shared addressing and privacy constraints for text and document requests.
#[derive(Default, Serialize)]
pub(crate) struct Delivery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<ChatId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_parameters: Option<Box<ReplyParameters>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_messages_topic_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral_message_parameters: Option<Box<EphemeralMessageParameters>>,
    #[serde(skip)]
    error: Option<&'static str>,
    #[serde(skip)]
    parent_date: Option<i64>,
}

impl Delivery {
    pub fn chat(chat_id: ChatId) -> Self {
        Self {
            chat_id: Some(chat_id),
            ..Self::default()
        }
    }
    pub fn callback(id: CallbackQueryId) -> Self {
        Self {
            ephemeral_message_parameters: Some(Box::new(
                EphemeralMessageParameters::new().callback_query_id(id),
            )),
            ..Self::default()
        }
    }
    #[cfg(feature = "bot")]
    pub fn reply(message: &crate::Message, guest: bool) -> Self {
        let mut scope = Self::chat(message.chat.id);
        if guest
            || message
                .guest_query_id
                .as_ref()
                .is_some_and(|id| !id.is_empty())
        {
            scope.error = Some("guest_query_id");
            return scope;
        }
        if let Some(id) = message.ephemeral_message_id {
            if message.chat.is_direct_messages.is_some() {
                scope.error = Some("ephemeral_chat");
                return scope;
            }
            let Some(receiver) = &message.receiver_user else {
                scope.error = Some("receiver_user_id");
                return scope;
            };
            scope.reply_parameters =
                Some(Box::new(ReplyParameters::new().ephemeral_message_id(id)));
            scope.ephemeral_message_parameters = Some(Box::new(
                EphemeralMessageParameters::new().receiver_user_id(receiver.id),
            ));
            scope.parent_date = Some(message.date);
        } else {
            scope.reply_parameters = Some(Box::new(
                ReplyParameters::new().message_id(message.message_id),
            ));
        }
        scope.message_thread_id = message.message_thread_id;
        scope.business_connection_id = message.business_connection_id.clone();
        scope.direct_messages_topic_id = message
            .direct_messages_topic
            .as_ref()
            .map(|topic| topic.topic_id);
        scope
    }

    /// Return the remaining ephemeral window, checked again when the request is awaited.
    pub fn validate(&self) -> Result<Option<Duration>> {
        if let Some(field) = self.error {
            return Err(invalid(field));
        }
        let timeout = if let Some(date) = self.parent_date {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::local(ErrorKind::Configuration, "clock"))?;
            if date < 0 || Duration::from_secs(date as u64) > now {
                return Err(invalid("ephemeral_message_expired"));
            }
            Some(
                Duration::from_secs(date as u64)
                    .checked_add(Duration::from_secs(15))
                    .and_then(|end| end.checked_sub(now))
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| invalid("ephemeral_message_expired"))?,
            )
        } else {
            None
        };
        if let Some(params) = &self.ephemeral_message_parameters
            && (params.receiver_user_id.is_none() && params.callback_query_id.is_none()
                || params.receiver_user_id.is_some_and(|id| i64::from(id) <= 0)
                || params
                    .callback_query_id
                    .as_ref()
                    .is_some_and(CallbackQueryId::is_empty))
        {
            return Err(invalid("ephemeral_message_parameters"));
        }
        if let Some(reply) = &self.reply_parameters
            && (reply.message_id.is_some() == reply.ephemeral_message_id.is_some()
                || reply.ephemeral_message_id.is_some()
                    && self.ephemeral_message_parameters.is_none())
        {
            return Err(invalid("reply_parameters"));
        }
        if self.chat_id.is_none()
            && self
                .ephemeral_message_parameters
                .as_ref()
                .is_none_or(|params| params.callback_query_id.is_none())
        {
            return Err(invalid("chat_id"));
        }
        Ok(timeout)
    }
}
fn invalid(field: &'static str) -> Error {
    Error::local(ErrorKind::Validation, field)
}
