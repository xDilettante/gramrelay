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
    context_error: Option<&'static str>,
    ephemeral_parent_date: Option<i64>,
}

#[derive(Serialize)]
struct MessageParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_id: Option<ChatId>,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entities: Vec<MessageEntity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_parameters: Option<Box<ReplyParameters>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    business_connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_messages_topic_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ephemeral_message_parameters: Option<Box<EphemeralMessageParameters>>,
}

impl std::fmt::Debug for SendMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendMessage").finish_non_exhaustive()
    }
}

impl<'a> SendMessage<'a> {
    pub(crate) fn new(bot: &'a Bot, chat_id: ChatId, text: RichText) -> Self {
        Self::with_target(bot, Some(chat_id), text)
    }
    pub(crate) fn for_callback(bot: &'a Bot, id: CallbackQueryId, text: RichText) -> Self {
        Self::with_target(bot, None, text)
            .ephemeral_message_parameters(EphemeralMessageParameters::new().callback_query_id(id))
    }
    fn with_target(bot: &'a Bot, chat_id: Option<ChatId>, text: RichText) -> Self {
        let (text, entities) = text.into_parts();
        Self {
            bot,
            context_error: None,
            ephemeral_parent_date: None,
            params: MessageParams {
                chat_id,
                text,
                entities,
                parse_mode: None,
                disable_notification: None,
                reply_markup: None,
                reply_parameters: None,
                message_thread_id: None,
                business_connection_id: None,
                direct_messages_topic_id: None,
                ephemeral_message_parameters: None,
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
    pub fn reply_to(mut self, id: MessageId) -> Self {
        self.params.reply_parameters = Some(Box::new(ReplyParameters::new().message_id(id)));
        self
    }
    pub fn reply_to_ephemeral(mut self, id: i64) -> Self {
        self.params.reply_parameters =
            Some(Box::new(ReplyParameters::new().ephemeral_message_id(id)));
        self
    }
    pub fn reply_parameters(mut self, params: ReplyParameters) -> Self {
        self.params.reply_parameters = Some(Box::new(params));
        self
    }
    pub fn direct_messages_topic_id(mut self, id: i64) -> Self {
        self.params.direct_messages_topic_id = Some(id);
        self
    }
    pub fn ephemeral_message_parameters(mut self, params: EphemeralMessageParameters) -> Self {
        self.params.ephemeral_message_parameters = Some(Box::new(params));
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
    pub(crate) fn invalid_context(mut self, field: &'static str) -> Self {
        self.context_error = Some(field);
        self
    }
    pub(crate) fn ephemeral_parent_date(mut self, date: i64) -> Self {
        self.ephemeral_parent_date = Some(date);
        self
    }
}

impl<'a> IntoFuture for SendMessage<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(field) = self.context_error {
                return Err(Error::local(ErrorKind::Validation, field));
            }
            if let Some(date) = self.ephemeral_parent_date {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| Error::local(ErrorKind::Configuration, "clock"))?
                    .as_secs();
                if date < 0 || now < date as u64 || now.saturating_sub(date as u64) >= 15 {
                    return Err(Error::local(
                        ErrorKind::Validation,
                        "ephemeral_message_expired",
                    ));
                }
            }
            if let Some(params) = &self.params.ephemeral_message_parameters
                && (params.receiver_user_id.is_none() && params.callback_query_id.is_none()
                    || params.receiver_user_id.is_some_and(|id| i64::from(id) <= 0)
                    || params
                        .callback_query_id
                        .as_ref()
                        .is_some_and(CallbackQueryId::is_empty))
            {
                return Err(Error::local(
                    ErrorKind::Validation,
                    "ephemeral_message_parameters",
                ));
            }
            if let Some(reply) = &self.params.reply_parameters
                && (reply.message_id.is_some() == reply.ephemeral_message_id.is_some()
                    || reply.ephemeral_message_id.is_some()
                        && self.params.ephemeral_message_parameters.is_none())
            {
                return Err(Error::local(ErrorKind::Validation, "reply_parameters"));
            }
            if self.params.chat_id.is_none()
                && self
                    .params
                    .ephemeral_message_parameters
                    .as_ref()
                    .is_none_or(|params| params.callback_query_id.is_none())
            {
                return Err(Error::local(ErrorKind::Validation, "chat_id"));
            }
            validate_content(&self.params.text, "text", 1, 4096, self.params.parse_mode)?;
            validate_parse_mode(self.params.parse_mode, &self.params.entities)?;
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
                .perform::<crate::api::True>(async {
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
    chat: ChatId,
    file: crate::InputFile,
    caption: Option<RichText>,
    parse_mode: Option<ParseMode>,
}
impl<'a> SendDocument<'a> {
    pub(crate) fn new(bot: &'a Bot, chat: ChatId, file: crate::InputFile) -> Self {
        Self {
            bot,
            chat,
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
}
impl<'a> IntoFuture for SendDocument<'a> {
    type Output = Result<Message>;
    type IntoFuture = RequestFuture<'a, Message>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(caption) = &self.caption {
                validate_content(caption.text(), "caption", 0, 1024, self.parse_mode)?;
                validate_parse_mode(self.parse_mode, caption.entities())?;
            }
            self.bot
                .perform(async move {
                    let mut form = reqwest::multipart::Form::new()
                        .text("chat_id", i64::from(self.chat).to_string());
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
                    let form = self.file.attach(form, self.bot.upload_limit()).await?;
                    Ok(self.bot.post("sendDocument")?.multipart(form))
                })
                .await
        })
    }
}
