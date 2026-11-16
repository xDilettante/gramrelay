use crate::{
    Bot, CallbackQueryId, ChatId, Error, ErrorKind, InlineKeyboardMarkup, Message, MessageId,
    Result,
};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_parameters: Option<ReplyParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    business_connection_id: Option<String>,
}

#[derive(Serialize)]
struct ReplyParams {
    message_id: MessageId,
}

impl<'a> SendMessage<'a> {
    pub(crate) fn new(bot: &'a Bot, chat_id: ChatId, text: String) -> Self {
        Self {
            bot,
            params: MessageParams {
                chat_id,
                text,
                disable_notification: None,
                reply_markup: None,
                reply_parameters: None,
                message_thread_id: None,
                business_connection_id: None,
            },
        }
    }
    pub fn disable_notification(mut self, value: bool) -> Self {
        self.params.disable_notification = Some(value);
        self
    }
    pub fn reply_markup(mut self, keyboard: InlineKeyboardMarkup) -> Self {
        self.params.reply_markup = Some(keyboard);
        self
    }
    pub fn reply_to(mut self, id: MessageId) -> Self {
        self.params.reply_parameters = Some(ReplyParams { message_id: id });
        self
    }
    pub fn message_thread_id(mut self, id: i64) -> Self {
        self.params.message_thread_id = Some(id);
        self
    }
    pub fn business_connection_id(mut self, id: impl Into<String>) -> Self {
        self.params.business_connection_id = Some(id.into());
        self
    }
}

impl<'a> IntoFuture for SendMessage<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            validate_text(&self.params.text, "text", 1, 4096)?;
            if let Some(keyboard) = &self.params.reply_markup {
                keyboard.validate()?;
            }
            self.bot
                .perform(async { Ok(self.bot.post("sendMessage")?.json(&self.params)) })
                .await
        })
    }
}

#[must_use = "requests are lazy; await the request to execute it"]
pub struct AnswerCallbackQuery<'a> {
    bot: &'a Bot,
    params: CallbackParams,
}

#[derive(Serialize)]
struct CallbackParams {
    callback_query_id: CallbackQueryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_alert: Option<bool>,
}
impl<'a> AnswerCallbackQuery<'a> {
    pub(crate) fn new(bot: &'a Bot, id: CallbackQueryId) -> Self {
        Self {
            bot,
            params: CallbackParams {
                callback_query_id: id,
                text: None,
                show_alert: None,
            },
        }
    }
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.params.text = Some(text.into());
        self
    }
    pub fn show_alert(mut self, show: bool) -> Self {
        self.params.show_alert = Some(show);
        self
    }
}
impl<'a> IntoFuture for AnswerCallbackQuery<'a> {
    type Output = Result<bool>;
    type IntoFuture = RequestFuture<'a, bool>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if self.params.callback_query_id.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "callback_query_id"));
            }
            if let Some(text) = &self.params.text {
                validate_text(text, "text", 0, 200)?;
            }
            self.bot
                .perform(async { Ok(self.bot.post("answerCallbackQuery")?.json(&self.params)) })
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

#[must_use = "requests are lazy; await the request to execute it"]
pub struct SendDocument<'a> {
    bot: &'a Bot,
    chat: ChatId,
    file: crate::InputFile,
    caption: Option<String>,
}
impl<'a> SendDocument<'a> {
    pub(crate) fn new(bot: &'a Bot, chat: ChatId, file: crate::InputFile) -> Self {
        Self {
            bot,
            chat,
            file,
            caption: None,
        }
    }
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}
impl<'a> IntoFuture for SendDocument<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(caption) = &self.caption {
                validate_text(caption, "caption", 0, 1024)?;
            }
            self.bot
                .perform(async move {
                    let mut form = reqwest::multipart::Form::new()
                        .text("chat_id", i64::from(self.chat).to_string());
                    if let Some(caption) = self.caption {
                        form = form.text("caption", caption);
                    }
                    let form = self.file.attach(form, self.bot.upload_limit()).await?;
                    Ok(self.bot.post("sendDocument")?.multipart(form))
                })
                .await
        })
    }
}
