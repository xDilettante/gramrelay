use crate::{
    AnswerCallbackQuery, Bot, CallbackQuery, Error, ErrorKind, Message, RequestFuture, Result,
    SendMessage, Update,
};
use std::{future::Future, sync::Arc};

type Handler<C> = Arc<dyn Fn(C) -> RequestFuture<'static, ()> + Send + Sync>;

enum Route<S> {
    Command(String, Handler<MessageContext<S>>),
    Message(MessageEvent, Handler<MessageContext<S>>),
    Callback(String, Handler<CallbackContext<S>>),
    AnyCallback(Handler<CallbackContext<S>>),
    Update(Handler<UpdateContext<S>>),
}

/// An explicit message-bearing update route. Polling subscriptions remain separately configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageEvent {
    Message,
    EditedMessage,
    ChannelPost,
    EditedChannelPost,
    BusinessMessage,
    EditedBusinessMessage,
    GuestMessage,
}
impl MessageEvent {
    fn take(self, update: &mut Update) -> Option<Message> {
        match self {
            Self::Message => update.message.take(),
            Self::EditedMessage => update.edited_message.take(),
            Self::ChannelPost => update.channel_post.take(),
            Self::EditedChannelPost => update.edited_channel_post.take(),
            Self::BusinessMessage => update.business_message.take(),
            Self::EditedBusinessMessage => update.edited_business_message.take(),
            Self::GuestMessage => update.guest_message.take(),
        }
        .map(|message| *message)
    }
}

/// Ordered routes with explicit shared state. Only the first matching handler runs.
pub struct Router<S = ()> {
    state: Arc<S>,
    username: Option<String>,
    routes: Vec<Route<S>>,
}

impl Default for Router<()> {
    fn default() -> Self {
        Self::new()
    }
}
impl Router<()> {
    pub fn new() -> Self {
        Self::with_state(())
    }
}

impl<S: Send + Sync + 'static> Router<S> {
    pub fn with_state(state: S) -> Self {
        Self {
            state: Arc::new(state),
            username: None,
            routes: Vec::new(),
        }
    }
    pub fn state(&self) -> &S {
        &self.state
    }
    /// For manual dispatch. Managed polling verifies this value using getMe.
    pub fn bot_username(mut self, name: impl Into<String>) -> Self {
        self.username = Some(name.into().to_ascii_lowercase());
        self
    }
    pub fn command<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(MessageContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes.push(Route::Command(
            name.into(),
            Arc::new(move |ctx| Box::pin(handler(ctx))),
        ));
        self
    }
    pub fn on_message<F, Fut>(self, handler: F) -> Self
    where
        F: Fn(MessageContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_message_event(MessageEvent::Message, handler)
    }
    pub fn on_message_event<F, Fut>(mut self, kind: MessageEvent, handler: F) -> Self
    where
        F: Fn(MessageContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes.push(Route::Message(
            kind,
            Arc::new(move |ctx| Box::pin(handler(ctx))),
        ));
        self
    }
    pub fn callback<F, Fut>(mut self, data: impl Into<String>, handler: F) -> Self
    where
        F: Fn(CallbackContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes.push(Route::Callback(
            data.into(),
            Arc::new(move |ctx| Box::pin(handler(ctx))),
        ));
        self
    }
    pub fn on_callback<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(CallbackContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes.push(Route::AnyCallback(Arc::new(move |ctx| {
            Box::pin(handler(ctx))
        })));
        self
    }
    pub fn on_update<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(UpdateContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes
            .push(Route::Update(Arc::new(move |ctx| Box::pin(handler(ctx)))));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self
            .username
            .as_ref()
            .is_some_and(|name| !valid_name(name, 64))
        {
            return Err(Error::local(ErrorKind::Validation, "bot_username"));
        }
        for route in &self.routes {
            match route {
                Route::Command(name, _) if !valid_name(name, 32) => {
                    return Err(Error::local(ErrorKind::Validation, "command"));
                }
                Route::Callback(data, _) if !(1..=64).contains(&data.len()) => {
                    return Err(Error::local(ErrorKind::Validation, "callback_data"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn bind_username(&mut self, name: Option<&str>) -> Result<()> {
        let actual = name.map(str::to_ascii_lowercase);
        if self.username.is_some() && self.username != actual {
            return Err(Error::local(ErrorKind::Configuration, "bot_username"));
        }
        self.username = actual;
        self.validate()
    }

    /// Dispatch one event. Manual ingestion owns concurrency and acknowledgements.
    /// Unmatched events are intentionally ignored.
    pub async fn dispatch(&self, bot: &Bot, mut update: Update) -> Result<()> {
        self.validate()?;
        if update.known_event_count() > 1 {
            return Err(Error::local(ErrorKind::Validation, "update_kind"));
        }
        for route in &self.routes {
            match route {
                Route::Command(name, handler) if self.matches_command(&update, name) => {
                    if let Some(message) = update.message.take() {
                        return handler(MessageContext {
                            message: *message,
                            event: MessageEvent::Message,
                            bot: bot.clone(),
                            state: self.state.clone(),
                        })
                        .await;
                    }
                }
                Route::Message(kind, handler) => {
                    if let Some(message) = kind.take(&mut update) {
                        return handler(MessageContext {
                            message,
                            event: *kind,
                            bot: bot.clone(),
                            state: self.state.clone(),
                        })
                        .await;
                    }
                }
                Route::Callback(data, handler)
                    if update
                        .callback_query
                        .as_ref()
                        .is_some_and(|query| query.data.as_deref() == Some(data)) =>
                {
                    if let Some(query) = update.callback_query.take() {
                        return handler(CallbackContext {
                            query: *query,
                            bot: bot.clone(),
                            state: self.state.clone(),
                        })
                        .await;
                    }
                }
                Route::AnyCallback(handler) => {
                    if let Some(query) = update.callback_query.take() {
                        return handler(CallbackContext {
                            query: *query,
                            bot: bot.clone(),
                            state: self.state.clone(),
                        })
                        .await;
                    }
                }
                Route::Update(handler) => {
                    return handler(UpdateContext {
                        update,
                        bot: bot.clone(),
                        state: self.state.clone(),
                    })
                    .await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn matches_command(&self, update: &Update, expected: &str) -> bool {
        let Some((name, mention, _)) = update
            .message
            .as_ref()
            .and_then(|m| m.text.as_deref())
            .and_then(command)
        else {
            return false;
        };
        name == expected
            && match mention {
                None => true,
                Some(mentioned) => self
                    .username
                    .as_ref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(mentioned)),
            }
    }
}

fn valid_name(name: &str, max: usize) -> bool {
    (1..=max).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn command(text: &str) -> Option<(&str, Option<&str>, &str)> {
    let body = text.strip_prefix('/')?;
    let (token, args) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
    let (name, mention) = token
        .split_once('@')
        .map_or((token, None), |(name, mention)| (name, Some(mention)));
    Some((name, mention, args.trim_start()))
}

#[non_exhaustive]
pub struct MessageContext<S = ()> {
    pub message: Message,
    pub event: MessageEvent,
    bot: Bot,
    state: Arc<S>,
}
impl<S> MessageContext<S> {
    pub fn bot(&self) -> &Bot {
        &self.bot
    }
    pub fn state(&self) -> &S {
        &self.state
    }
    pub fn command_args(&self) -> Option<&str> {
        self.message
            .text
            .as_deref()
            .and_then(command)
            .map(|(_, _, args)| args)
    }
    /// Reply with the original message scope. Guest messages require answer_guest.
    pub fn reply(&self, text: impl Into<crate::RichText>) -> SendMessage<'_> {
        SendMessage::with_delivery(
            &self.bot,
            crate::delivery::Delivery::reply(
                &self.message,
                self.event == MessageEvent::GuestMessage,
            ),
            text.into(),
        )
    }
    pub fn reply_document(&self, file: crate::InputFile) -> crate::SendDocument<'_> {
        crate::SendDocument::with_delivery(
            &self.bot,
            crate::delivery::Delivery::reply(
                &self.message,
                self.event == MessageEvent::GuestMessage,
            ),
            file,
        )
    }
    pub fn answer_guest(
        &self,
        result: crate::api::types::InlineQueryResult,
    ) -> Result<crate::api::Call<'_, crate::api::methods::AnswerGuestQuery>> {
        let id = self
            .message
            .guest_query_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::local(ErrorKind::Validation, "guest_query_id"))?;
        Ok(self.bot.api().answer_guest_query(id.clone(), result))
    }
}

#[non_exhaustive]
pub struct CallbackContext<S = ()> {
    pub query: CallbackQuery,
    bot: Bot,
    state: Arc<S>,
}
impl<S> CallbackContext<S> {
    pub fn bot(&self) -> &Bot {
        &self.bot
    }
    pub fn state(&self) -> &S {
        &self.state
    }
    pub fn answer(&self) -> AnswerCallbackQuery<'_> {
        self.bot.answer_callback_query(self.query.id.clone())
    }
    pub fn reply(&self, text: impl Into<crate::RichText>) -> Result<SendMessage<'_>> {
        let message = self
            .query
            .message
            .as_ref()
            .and_then(|message| message.accessible())
            .ok_or_else(|| Error::local(ErrorKind::Validation, "callback_message"))?;
        Ok(SendMessage::with_delivery(
            &self.bot,
            crate::delivery::Delivery::reply(message, false),
            text.into(),
        ))
    }
    pub fn reply_document(&self, file: crate::InputFile) -> Result<crate::SendDocument<'_>> {
        let message = self
            .query
            .message
            .as_ref()
            .and_then(|message| message.accessible())
            .ok_or_else(|| Error::local(ErrorKind::Validation, "callback_message"))?;
        Ok(crate::SendDocument::with_delivery(
            &self.bot,
            crate::delivery::Delivery::reply(message, false),
            file,
        ))
    }
    pub fn ephemeral_reply_document(
        &self,
        file: crate::InputFile,
    ) -> Result<crate::SendDocument<'_>> {
        if self
            .query
            .message
            .as_ref()
            .is_some_and(|message| message.chat().is_direct_messages.is_some())
        {
            return Err(Error::local(ErrorKind::Validation, "ephemeral_chat"));
        }
        Ok(self
            .bot
            .reply_ephemeral_document(self.query.id.clone(), file))
    }
    /// Send to the callback's caller; answer() is still required separately.
    pub fn ephemeral_reply(&self, text: impl Into<crate::RichText>) -> Result<SendMessage<'_>> {
        if self
            .query
            .message
            .as_ref()
            .is_some_and(|message| message.chat().is_direct_messages.is_some())
        {
            return Err(Error::local(ErrorKind::Validation, "ephemeral_chat"));
        }
        Ok(self.bot.reply_ephemeral(self.query.id.clone(), text))
    }
}

#[non_exhaustive]
pub struct UpdateContext<S = ()> {
    pub update: Update,
    bot: Bot,
    state: Arc<S>,
}
impl<S> UpdateContext<S> {
    pub fn bot(&self) -> &Bot {
        &self.bot
    }
    pub fn state(&self) -> &S {
        &self.state
    }
}
