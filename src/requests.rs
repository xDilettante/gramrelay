use crate::{
    Bot, CallbackQueryId, ChatId, Error, ErrorKind, InlineKeyboardMarkup, Message, MessageId,
    ParseMode, Result, RichText,
    api::types::{EphemeralMessageParameters, MessageEntity, ReplyParameters},
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
    #[serde(flatten)]
    delivery: crate::delivery::Delivery,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entities: Vec<MessageEntity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<InlineKeyboardMarkup>,
}
impl std::fmt::Debug for SendMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendMessage").finish_non_exhaustive()
    }
}
impl<'a> SendMessage<'a> {
    pub(crate) fn new(bot: &'a Bot, chat: ChatId, text: RichText) -> Self {
        Self::with_delivery(bot, crate::delivery::Delivery::chat(chat), text)
    }
    pub(crate) fn for_callback(bot: &'a Bot, id: CallbackQueryId, text: RichText) -> Self {
        Self::with_delivery(bot, crate::delivery::Delivery::callback(id), text)
    }
    pub(crate) fn with_delivery(
        bot: &'a Bot,
        delivery: crate::delivery::Delivery,
        text: RichText,
    ) -> Self {
        let (text, entities) = text.into_parts();
        Self {
            bot,
            params: MessageParams {
                delivery,
                text,
                entities,
                parse_mode: None,
                disable_notification: None,
                reply_markup: None,
            },
        }
    }
    pub fn disable_notification(mut self, value: bool) -> Self {
        self.params.disable_notification = Some(value);
        self
    }
    pub fn parse_mode(mut self, value: ParseMode) -> Self {
        self.params.parse_mode = Some(value);
        self
    }
    pub fn reply_markup(mut self, keyboard: InlineKeyboardMarkup) -> Self {
        self.params.reply_markup = Some(keyboard);
        self
    }
    pub fn reply_to(self, id: MessageId) -> Self {
        self.reply_parameters(ReplyParameters::new().message_id(id))
    }
    pub fn reply_to_ephemeral(self, id: i64) -> Self {
        self.reply_parameters(ReplyParameters::new().ephemeral_message_id(id))
    }
    pub fn reply_parameters(mut self, params: ReplyParameters) -> Self {
        self.params.delivery.reply_parameters = Some(Box::new(params));
        self
    }
    pub fn direct_messages_topic_id(mut self, id: i64) -> Self {
        self.params.delivery.direct_messages_topic_id = Some(id);
        self
    }
    pub fn ephemeral_message_parameters(mut self, params: EphemeralMessageParameters) -> Self {
        self.params.delivery.ephemeral_message_parameters = Some(Box::new(params));
        self
    }
    pub fn message_thread_id(mut self, id: i64) -> Self {
        self.params.delivery.message_thread_id = Some(id);
        self
    }
    pub fn business_connection_id(mut self, id: impl Into<String>) -> Self {
        self.params.delivery.business_connection_id = Some(id.into());
        self
    }
}
impl<'a> IntoFuture for SendMessage<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.params.delivery.validate()?;
            validate_content(&self.params.text, "text", 1, 4096, self.params.parse_mode)?;
            validate_parse_mode(self.params.parse_mode, &self.params.entities)?;
            if let Some(keyboard) = &self.params.reply_markup {
                keyboard.validate()?;
            }
            self.bot
                .perform_with_timeout(
                    "sendMessage",
                    async { Ok(self.bot.post("sendMessage")?.json(&self.params)) },
                    timeout,
                )
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
                .perform::<crate::api::True>("answerCallbackQuery", async {
                    Ok(self.bot.post("answerCallbackQuery")?.json(&self.params))
                })
                .await
                .map(|_| true)
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

fn validate_parse_mode(mode: Option<ParseMode>, entities: &[MessageEntity]) -> Result<()> {
    if mode.is_some() && !entities.is_empty() {
        return Err(Error::local(ErrorKind::Validation, "parse_mode"));
    }
    Ok(())
}

fn validate_content(
    text: &str,
    field: &'static str,
    min: usize,
    max: usize,
    mode: Option<ParseMode>,
) -> Result<()> {
    // Explicit markup is parsed on Telegram; its source length isn't the rendered length.
    validate_text(
        text,
        field,
        min,
        if mode.is_some() { usize::MAX } else { max },
    )
}

#[must_use = "requests are lazy; await the request to execute it"]
pub struct SendDocument<'a> {
    bot: &'a Bot,
    delivery: crate::delivery::Delivery,
    file: crate::InputFile,
    caption: Option<RichText>,
    parse_mode: Option<ParseMode>,
}
impl std::fmt::Debug for SendDocument<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendDocument").finish_non_exhaustive()
    }
}
impl<'a> SendDocument<'a> {
    pub(crate) fn new(bot: &'a Bot, chat: ChatId, file: crate::InputFile) -> Self {
        Self::with_delivery(bot, crate::delivery::Delivery::chat(chat), file)
    }
    pub(crate) fn with_delivery(
        bot: &'a Bot,
        delivery: crate::delivery::Delivery,
        file: crate::InputFile,
    ) -> Self {
        Self {
            bot,
            delivery,
            file,
            caption: None,
            parse_mode: None,
        }
    }
    pub fn caption(mut self, caption: impl Into<RichText>) -> Self {
        self.caption = Some(caption.into());
        self
    }
    pub fn parse_mode(mut self, value: ParseMode) -> Self {
        self.parse_mode = Some(value);
        self
    }
    pub fn reply_to(self, id: MessageId) -> Self {
        self.reply_parameters(ReplyParameters::new().message_id(id))
    }
    pub fn reply_to_ephemeral(self, id: i64) -> Self {
        self.reply_parameters(ReplyParameters::new().ephemeral_message_id(id))
    }
    pub fn reply_parameters(mut self, params: ReplyParameters) -> Self {
        self.delivery.reply_parameters = Some(Box::new(params));
        self
    }
    pub fn direct_messages_topic_id(mut self, id: i64) -> Self {
        self.delivery.direct_messages_topic_id = Some(id);
        self
    }
    pub fn ephemeral_message_parameters(mut self, params: EphemeralMessageParameters) -> Self {
        self.delivery.ephemeral_message_parameters = Some(Box::new(params));
        self
    }
    pub fn message_thread_id(mut self, id: i64) -> Self {
        self.delivery.message_thread_id = Some(id);
        self
    }
    pub fn business_connection_id(mut self, id: impl Into<String>) -> Self {
        self.delivery.business_connection_id = Some(id.into());
        self
    }
}
impl<'a> IntoFuture for SendDocument<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let timeout = self.delivery.validate()?;
            if let Some(caption) = &self.caption {
                validate_content(caption.text(), "caption", 0, 1024, self.parse_mode)?;
                validate_parse_mode(self.parse_mode, caption.entities())?;
            }
            self.bot
                .perform_upload(
                    "sendDocument",
                    async move {
                        let delivery = serde_json::to_value(&self.delivery)
                            .map_err(|_| Error::local(ErrorKind::Validation, "delivery"))?;
                        let mut form = reqwest::multipart::Form::new();
                        for (name, value) in delivery
                            .as_object()
                            .expect("Delivery serializes as an object")
                        {
                            let text = value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string());
                            form = form.text(name.clone(), text);
                        }
                        if let Some(caption) = self.caption {
                            let (caption, entities) = caption.into_parts();
                            form = form.text("caption", caption);
                            if !entities.is_empty() {
                                let entities = serde_json::to_string(&entities).map_err(|_| {
                                    Error::local(ErrorKind::Validation, "caption_entities")
                                })?;
                                form = form.text("caption_entities", entities);
                            }
                        }
                        if let Some(mode) = self.parse_mode {
                            let mode = serde_json::to_value(mode)
                                .map_err(|_| Error::local(ErrorKind::Validation, "parse_mode"))?;
                            form = form.text("parse_mode", mode.as_str().unwrap().to_owned());
                        }
                        let mut readers = crate::files::UploadReaders::default();
                        let form = self
                            .file
                            .attach(form, self.bot.upload_limit(), &mut readers)
                            .await?;
                        Ok((self.bot.post("sendDocument")?.multipart(form), readers))
                    },
                    timeout,
                )
                .await
        })
    }
}
