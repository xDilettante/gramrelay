use super::*;
use crate::updates::{StoredUpdate, UpdateCursor};
use serde::{Serializer, ser::Error as _};

#[derive(Clone, Serialize)]
pub struct BotInfo {
    bot_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}
impl BotInfo {
    pub fn bot_id(&self) -> &str {
        &self.bot_id
    }
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }
}
#[derive(Serialize)]
pub struct ChatInfo {
    chat_id: ChatId,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}
impl ChatInfo {
    pub fn chat_id(&self) -> ChatId {
        self.chat_id
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }
}
/// Primary event scope, explicitly reviewed separately from dispatch ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "chat_id", rename_all = "snake_case")]
pub enum ReadEventScope {
    Chat(ChatId),
    Unscoped,
}
#[derive(Serialize)]
pub struct ReadEvent {
    cursor: UpdateCursor,
    update_id: String,
    scope: ReadEventScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callback_query_id: Option<String>,
    received_at_ms: i64,
    untrusted: bool,
    #[serde(rename = "telegram_json", serialize_with = "telegram_json")]
    update: tg::Update,
}
impl ReadEvent {
    pub fn cursor(&self) -> &UpdateCursor {
        &self.cursor
    }
    pub fn scope(&self) -> ReadEventScope {
        self.scope
    }
    pub fn update(&self) -> &tg::Update {
        &self.update
    }
    pub fn chat_id(&self) -> Option<ChatId> {
        match self.scope {
            ReadEventScope::Chat(chat) => Some(chat),
            ReadEventScope::Unscoped => None,
        }
    }
    pub fn message_id(&self) -> Option<MessageId> {
        self.message_id
    }
    pub fn callback_query_id(&self) -> Option<&str> {
        self.callback_query_id.as_deref()
    }
    pub fn received_at_ms(&self) -> i64 {
        self.received_at_ms
    }
    fn new(event: StoredUpdate, scope: ReadEventScope, callback_query_id: Option<String>) -> Self {
        let cursor = event.cursor().clone();
        let received_at_ms = event.received_at_ms();
        let update = event.into_update();
        let message = primary_message(&update);
        let message_id = if matches!(scope, ReadEventScope::Chat(_)) {
            message
                .map(|v| v.message_id)
                .or_else(|| {
                    update
                        .callback_query
                        .as_ref()
                        .and_then(|v| v.message.as_ref())
                        .map(|v| v.message_id())
                })
                .or_else(|| update.message_reaction.as_ref().map(|v| v.message_id))
                .or_else(|| update.message_reaction_count.as_ref().map(|v| v.message_id))
                .and_then(|v| MessageId::new(i64::from(v)).ok())
        } else {
            None
        };
        let message_thread_id = if matches!(scope, ReadEventScope::Chat(_)) {
            message
                .and_then(|v| v.message_thread_id)
                .or_else(|| {
                    update
                        .stopped_message_generation
                        .as_ref()
                        .and_then(|v| v.message_thread_id)
                })
                .and_then(|v| MessageId::new(v).ok())
        } else {
            None
        };
        Self {
            cursor,
            update_id: update.update_id.to_string(),
            scope,
            message_id,
            message_thread_id,
            callback_query_id,
            received_at_ms,
            untrusted: true,
            update,
        }
    }
}
#[derive(Serialize)]
pub struct ReadPage {
    events: Vec<ReadEvent>,
    next_cursor: UpdateCursor,
    has_more: bool,
}
impl ReadPage {
    pub fn events(&self) -> &[ReadEvent] {
        &self.events
    }
    pub fn next_cursor(&self) -> &UpdateCursor {
        &self.next_cursor
    }
    pub fn has_more(&self) -> bool {
        self.has_more
    }
}
fn telegram_json<S: Serializer>(
    update: &tg::Update,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(
        &serde_json::to_string(update).map_err(|_| S::Error::custom("invalid stored update"))?,
    )
}

impl Executor {
    /// Resolves immutable reference metadata only; no RPC, quota or state refresh.
    pub async fn dry_run(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<DryRun> {
        supported(request)?;
        let profile = self.profile_for(request)?;
        let auth = self
            .read_authorization(&profile, principal, request, approval)
            .await?;
        auth.checked(now()?).map(|(dry, _)| dry)
    }
    /// Explicit host confirmation, including owner-checked reference scope.
    pub async fn approve(
        &self,
        principal: &Principal,
        request: &Request,
        expires_at_ms: i64,
    ) -> Result<Approval> {
        supported(request)?;
        let profile = self.profile_for(request)?;
        let auth = self
            .read_authorization(&profile, principal, request, None)
            .await?;
        if let Some(reference) = &auth.reference {
            let time = now()?;
            let remaining = reference.remaining(time)?;
            if expires_at_ms > reference.expires_at_ms
                || expires_at_ms.saturating_sub(time) > remaining.as_millis() as i64
            {
                return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy));
            }
        }
        let approval = self.policy.approve_with_scope(
            principal,
            request,
            profile.bot_id,
            PolicyTime::Current(now()?),
            expires_at_ms,
            auth.scope.as_ref(),
        )?;
        if let Some(reference) = &auth.reference {
            reference.remaining(now()?)?;
        }
        Ok(approval)
    }
    async fn resolve_scope(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
    ) -> Result<Option<OperationScope>> {
        if let Some(cursor) = validate_cursor(request)? {
            self.policy
                .check_lookup(principal, request, profile.bot_id, now()?)?;
            profile
                .feed
                .check_cursor_position(&cursor)
                .await
                .map_err(storage_error)?;
        }
        let ActionCall::GetOperation(args) = request.call() else {
            return Ok(None);
        };
        self.policy
            .check_lookup(principal, request, profile.bot_id, now()?)?;
        profile
            .journal
            .operation_scope(request.profile(), principal.as_str(), &args.operation_id)
            .await
            .map_err(storage_error)?
            .map(Some)
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))
    }
    pub(super) async fn read_authorization(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<Authorization> {
        let file = if let ActionCall::SendDocument(args) = request.call() {
            self.policy
                .check_lookup(principal, request, profile.bot_id, now()?)?;
            Some(
                profile
                    .journal
                    .file_metadata(request.profile(), principal.as_str(), &args.file_handle)
                    .await
                    .map_err(storage_error)?
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?,
            )
        } else {
            None
        };
        let (scope, reference) = if request.action() == Action::AnswerCallbackQuery {
            let (scope, reference) = self.callback_context(profile, principal, request)?;
            (Some(scope), Some(reference))
        } else {
            (self.resolve_scope(profile, principal, request).await?, None)
        };
        Ok(Authorization {
            policy: self.policy.clone(),
            principal: principal.clone(),
            request: request.clone(),
            approval: approval.cloned(),
            bot_id: profile.bot_id,
            scope,
            reference,
            file,
        })
    }
    pub(super) async fn verified<'a>(
        &self,
        profile: &'a Profile,
        auth: &Authorization,
    ) -> Result<&'a BotInfo> {
        profile
            .identity
            .get_or_try_init(|| async {
                auth.write_checked(now()?)?;
                let identity =
                    timeout(self.options.request_budget, profile.bot.verified_identity())
                        .await
                        .map_err(|_| ExecutorError::new(ExecutorErrorKind::Identity))?
                        .map_err(|_| ExecutorError::new(ExecutorErrorKind::Identity))?;
                if i64::from(identity.id) != profile.bot_id {
                    return Err(ExecutorError::new(ExecutorErrorKind::Identity));
                }
                if identity
                    .username
                    .as_ref()
                    .is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control))
                {
                    return Err(invalid_response());
                }
                Ok(BotInfo {
                    bot_id: profile.bot_id.to_string(),
                    username: identity.username,
                })
            })
            .await
    }
    pub(super) async fn execute_read(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let auth = self
            .read_authorization(profile, principal, request, approval)
            .await?;
        auth.checked(now()?)?;
        let invocation = auth.clone();
        profile
            .journal
            .charge_authorized_invocation(move |time| {
                let context = invocation.budget_at(time)?;
                let cost = context.cost;
                Ok((context, cost))
            })
            .await
            .map_err(storage_error)?;
        auth.checked(now()?)?;
        match request.call() {
            ActionCall::GetMe(_) => {
                let bot = self.verified(profile, &auth).await?.clone();
                auth.checked(now()?)?;
                Ok(ExecutionResult {
                    schema_version: 1,
                    value: ExecutionValue::Bot { bot },
                })
            }
            ActionCall::GetChat(args) => {
                self.verified(profile, &auth).await?;
                auth.checked(now()?)?;
                let params =
                    tg::api::methods::GetChat::new(tg::ChatId::from(args.chat_id.get()).into());
                let result = timeout(
                    self.options.request_budget,
                    profile.bot.call(params).retry_policy(
                        tg::RetryPolicy::default()
                            .max_attempts(1)
                            .max_elapsed(self.options.request_budget),
                    ),
                )
                .await
                .map_err(|_| ExecutorError::new(ExecutorErrorKind::Remote))?
                .map_err(|_| ExecutorError::new(ExecutorErrorKind::Remote))?;
                auth.checked(now()?)?;
                if i64::from(result.id) != args.chat_id.get()
                    || result.kind.is_empty()
                    || result.kind.len() > 32
                    || result.kind.chars().any(char::is_control)
                    || result.title.as_ref().is_some_and(|v| v.len() > 1024)
                    || result.username.as_ref().is_some_and(|v| v.len() > 128)
                {
                    return Err(invalid_response());
                }
                Ok(ExecutionResult {
                    schema_version: 1,
                    value: ExecutionValue::Chat {
                        chat: ChatInfo {
                            chat_id: args.chat_id,
                            kind: result.kind,
                            title: result.title,
                            username: result.username,
                        },
                    },
                })
            }
            ActionCall::GetOperation(args) => {
                let (operation, scope) = profile
                    .journal
                    .get_scoped(request.profile(), principal.as_str(), &args.operation_id)
                    .await
                    .map_err(storage_error)?
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
                auth.checked(now()?)?;
                if auth.scope.as_ref() != Some(&scope) {
                    return Err(ExecutorError::new(ExecutorErrorKind::Denied));
                }
                project(operation, &scope)
            }
            ActionCall::ReadUpdates(args) => {
                let cursor = validate_cursor(request)?;
                let page = profile
                    .feed
                    .read_with_budget(cursor.as_ref(), 100, self.options.max_read_bytes)
                    .await
                    .map_err(storage_error)?;
                let (checked, grant) = auth.checked(now()?)?;
                let mut next_cursor = page.next_cursor().clone();
                let page_has_more = page.has_more();
                let count = page.events().len();
                let mut considered = 0usize;
                let mut selected = Vec::new();
                for event in page.into_events() {
                    let scope = event_scope(event.update());
                    considered += 1;
                    next_cursor = event.cursor().clone();
                    if permits(&grant, scope, event.update()) {
                        selected.push((event, scope));
                        if selected.len() == args.limit as usize {
                            break;
                        }
                    }
                }
                let mut events = Vec::with_capacity(selected.len());
                {
                    let mut registry = self
                        .callback_references
                        .lock()
                        .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?;
                    let time = now()?;
                    registry.prune(time);
                    for (event, scope) in selected {
                        let id = if event.update().callback_query.is_some() {
                            registry
                                .register_event(
                                    profile,
                                    principal,
                                    request.profile(),
                                    &event,
                                    &self.options,
                                    time,
                                )
                                .ok()
                                .map(|reference| reference.callback_query_id().to_owned())
                        } else {
                            None
                        };
                        events.push(ReadEvent::new(event, scope, id));
                    }
                }
                let (current, _) = auth.checked(now()?)?;
                if checked.grant_revision != current.grant_revision {
                    return Err(ExecutorError::new(ExecutorErrorKind::Denied));
                }
                Ok(ExecutionResult {
                    schema_version: 1,
                    value: ExecutionValue::Updates {
                        updates: ReadPage {
                            events,
                            next_cursor,
                            has_more: page_has_more || considered < count,
                        },
                    },
                })
            }
            _ => Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction)),
        }
    }
}
fn invalid_response() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidResponse)
}
pub(crate) fn validate_cursor(request: &Request) -> Result<Option<UpdateCursor>> {
    let ActionCall::ReadUpdates(args) = request.call() else {
        return Ok(None);
    };
    let cursor = args
        .cursor
        .as_deref()
        .map(str::parse::<UpdateCursor>)
        .transpose()
        .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidInput))?;
    if cursor
        .as_ref()
        .is_some_and(|v| !v.matches_profile(request.profile()))
    {
        return Err(ExecutorError::new(ExecutorErrorKind::Denied));
    }
    Ok(cursor)
}
fn permits(grant: &Grant, scope: ReadEventScope, update: &tg::Update) -> bool {
    match scope {
        ReadEventScope::Chat(chat) => grant.permits_chat(chat),
        // Unknown/ambiguous extensions require the global permission in
        // addition to any recognizable ordinary chat boundary they contain.
        ReadEventScope::Unscoped => {
            grant.permits_unscoped_updates()
                && primary_chat(update).is_none_or(|raw| {
                    ChatId::new(i64::from(raw)).is_ok_and(|chat| grant.permits_chat(chat))
                })
        }
    }
}
fn primary_message(update: &tg::Update) -> Option<&tg::Message> {
    [
        update.message.as_deref(),
        update.edited_message.as_deref(),
        update.channel_post.as_deref(),
        update.edited_channel_post.as_deref(),
        update.business_message.as_deref(),
        update.edited_business_message.as_deref(),
    ]
    .into_iter()
    .flatten()
    .next()
    .or_else(|| {
        update
            .callback_query
            .as_ref()
            .and_then(|v| v.message.as_ref())
            .and_then(|v| v.accessible())
    })
}
fn event_scope(update: &tg::Update) -> ReadEventScope {
    if update.known_event_count() != 1
        || !update.unknown_fields().is_empty()
        || ambiguous_callback(update)
    {
        return ReadEventScope::Unscoped;
    }
    primary_chat(update)
        .and_then(|raw| ChatId::new(i64::from(raw)).ok())
        .map_or(ReadEventScope::Unscoped, ReadEventScope::Chat)
}
pub(super) fn ambiguous_callback(update: &tg::Update) -> bool {
    update
        .callback_query
        .as_ref()
        .is_some_and(|value| value.message.is_some() && value.inline_message_id.is_some())
}
fn primary_chat(update: &tg::Update) -> Option<tg::ChatId> {
    primary_message(update)
        .map(|v| v.chat.id)
        .or_else(|| {
            update
                .callback_query
                .as_ref()
                .and_then(|v| v.message.as_ref())
                .map(|v| v.chat().id)
        })
        .or_else(|| {
            [
                update.deleted_business_messages.as_ref().map(|v| v.chat.id),
                update.message_reaction.as_ref().map(|v| v.chat.id),
                update.message_reaction_count.as_ref().map(|v| v.chat.id),
                update.my_chat_member.as_ref().map(|v| v.chat.id),
                update.chat_member.as_ref().map(|v| v.chat.id),
                update.chat_join_request.as_ref().map(|v| v.chat.id),
                update.chat_boost.as_ref().map(|v| v.chat.id),
                update.removed_chat_boost.as_ref().map(|v| v.chat.id),
                update
                    .stopped_message_generation
                    .as_ref()
                    .map(|v| v.chat.id),
            ]
            .into_iter()
            .flatten()
            .next()
        })
}
macro_rules! redacted_debug {
    ($($name:ident),+ $(,)?) => { $(impl fmt::Debug for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name),"([REDACTED])")) } })+ };
}
redacted_debug!(BotInfo, ChatInfo, ReadEvent, ReadPage);
