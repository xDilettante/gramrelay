use crate::{
    AnswerCallbackQuery, Bot, CallbackQuery, Error, ErrorKind, Message, RequestFuture, Result,
    SendMessage, Update,
};
use std::{future::Future, sync::Arc};

type Handler<C> = Arc<dyn Fn(C) -> RequestFuture<'static, ()> + Send + Sync>;

enum Route<S> {
    Command(String, Handler<MessageContext<S>>),
    Message(Handler<MessageContext<S>>),
    Callback(String, Handler<CallbackContext<S>>),
    AnyCallback(Handler<CallbackContext<S>>),
    Update(Handler<UpdateContext<S>>),
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
    pub fn on_message<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(MessageContext<S>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.routes
            .push(Route::Message(Arc::new(move |ctx| Box::pin(handler(ctx)))));
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
                            bot: bot.clone(),
                            state: self.state.clone(),
                        })
                        .await;
                    }
                }
                Route::Message(handler) => {
                    if let Some(message) = update.message.take() {
                        return handler(MessageContext {
                            message: *message,
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
    /// Reply in the originating message's chat, topic and business connection.
    pub fn reply(&self, text: impl Into<crate::RichText>) -> SendMessage<'_> {
        let mut request = self
            .bot
            .send_message(self.message.chat.id, text)
            .reply_to(self.message.message_id);
        if let Some(thread) = self.message.message_thread_id {
            request = request.message_thread_id(thread);
        }
        if let Some(connection) = &self.message.business_connection_id {
            request = request.business_connection_id(connection);
        }
        request
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
