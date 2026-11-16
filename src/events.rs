use crate::{Chat, ChatId, Message, MessageId, Update, api::types::MaybeInaccessibleMessage};

impl MaybeInaccessibleMessage {
    pub fn chat(&self) -> &Chat {
        match self {
            Self::Message(message) => &message.chat,
            Self::InaccessibleMessage(message) => &message.chat,
        }
    }
    pub fn message_id(&self) -> MessageId {
        match self {
            Self::Message(message) => message.message_id,
            Self::InaccessibleMessage(message) => message.message_id,
        }
    }
    pub fn accessible(&self) -> Option<&Message> {
        match self {
            Self::Message(message) => Some(message),
            Self::InaccessibleMessage(_) => None,
        }
    }
}

impl Update {
    /// A reliable event chat scope. Account/inline/unknown or ambiguous events have no single scope.
    pub fn chat_id(&self) -> Option<ChatId> {
        if self.known_event_count() != 1 {
            return None;
        }
        if let Some(message) = [
            self.message.as_deref(),
            self.edited_message.as_deref(),
            self.channel_post.as_deref(),
            self.edited_channel_post.as_deref(),
            self.business_message.as_deref(),
            self.edited_business_message.as_deref(),
            self.guest_message.as_deref(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            return Some(message.chat.id);
        }
        if let Some(callback) = &self.callback_query {
            return callback.message.as_ref().map(|message| message.chat().id);
        }
        if let Some(widget) = &self.widget_query {
            return widget.chat.as_ref().map(|chat| chat.id);
        }
        [
            self.deleted_business_messages
                .as_ref()
                .map(|event| event.chat.id),
            self.message_reaction.as_ref().map(|event| event.chat.id),
            self.message_reaction_count
                .as_ref()
                .map(|event| event.chat.id),
            self.my_chat_member.as_ref().map(|event| event.chat.id),
            self.chat_member.as_ref().map(|event| event.chat.id),
            self.chat_join_request.as_ref().map(|event| event.chat.id),
            self.chat_boost.as_ref().map(|event| event.chat.id),
            self.removed_chat_boost.as_ref().map(|event| event.chat.id),
            self.stopped_message_generation
                .as_ref()
                .map(|event| event.chat.id),
        ]
        .into_iter()
        .flatten()
        .next()
    }
}
