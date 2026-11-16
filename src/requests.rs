use crate::{Bot, ChatId, Error, ErrorKind, Message, Result};
use serde::Serialize;
use std::{
    future::{Future, IntoFuture},
    pin::Pin,
};

/// ponytail: one boxed future per request; replace only if measured allocation cost warrants it.
pub type RequestFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[must_use = "requests are lazy; await the request to execute it"]
pub struct SendMessage<'a> {
    bot: &'a Bot,
    params: MessageParams,
}

#[derive(Serialize)]
struct MessageParams {
    chat_id: ChatId,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_notification: Option<bool>,
}

impl<'a> SendMessage<'a> {
    pub(crate) fn new(bot: &'a Bot, chat_id: ChatId, text: String) -> Self {
        Self {
            bot,
            params: MessageParams {
                chat_id,
                text,
                disable_notification: None,
            },
        }
    }
    pub fn disable_notification(mut self, value: bool) -> Self {
        self.params.disable_notification = Some(value);
        self
    }
}

impl<'a> IntoFuture for SendMessage<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            validate_text(&self.params.text, "text", 1, 4096)?;
            self.bot
                .perform(async { Ok(self.bot.post("sendMessage")?.json(&self.params)) })
                .await
        })
    }
}

pub(crate) fn validate_text(text: &str, field: &'static str, min: usize, max: usize) -> Result<()> {
    let count = text.chars().count();
    if !(min..=max).contains(&count) {
        return Err(Error::local(ErrorKind::Validation, field));
    }
    Ok(())
}
