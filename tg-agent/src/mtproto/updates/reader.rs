use super::*;
use crate::{
    executor::{ExecutorError, ExecutorErrorKind, PolicyTime, storage_error},
    journal::authorization::{BudgetContext, charge_invocation_checked},
    mtproto::{
        Approval, Call, DryRun, Grant, PolicyStore, Principal, Request,
        types::Result as AgentResult,
    },
};
use serde::Serialize;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct ReadOptions {
    max_in_flight: usize,
    max_read_bytes: usize,
}
impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            max_in_flight: 16,
            max_read_bytes: 1024 * 1024,
        }
    }
}
impl ReadOptions {
    /// Bounds read jobs, including native work retained after caller cancellation.
    pub fn max_in_flight(mut self, value: usize) -> Self {
        self.max_in_flight = value;
        self
    }
    /// Bounds stored payload/query bytes inspected, before scope filtering.
    pub fn max_read_bytes(mut self, value: usize) -> Self {
        self.max_read_bytes = value;
        self
    }
}

/// Host-bound authorized durable reads. No RPC or callback capability issuance.
///
/// The host supplies the feed, current policy and authenticated principal.
/// Requests cannot change that authority. Cancellation may still charge quota.
/// Construct one reader per configured feed and clone it to share read limits.
///
/// ```no_run
/// use tg_agent::{executor::ExecutorError, mtproto::{PolicyStore, Principal, Request,
///     updates::{Feed, Reader, ReadOptions, ReadPage}}};
/// async fn read_page(feed: Feed, policy: PolicyStore, principal: &Principal,
///     request: &Request) -> Result<ReadPage, ExecutorError> {
///     let reader = Reader::new(feed, policy, ReadOptions::default())?;
///     reader.read(principal, request, None).await
/// }
/// ```
#[derive(Clone)]
pub struct Reader {
    feed: Feed,
    policy: PolicyStore,
    options: ReadOptions,
    slots: Arc<Semaphore>,
}
impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoReader([REDACTED])")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "peer", rename_all = "snake_case")]
pub enum ReadScope {
    Peer(PeerId),
    Unscoped,
}
#[derive(Serialize)]
pub struct ReadEvent {
    cursor: Cursor,
    scope: ReadScope,
    received_at_ms: i64,
    checkpoint_acknowledged: bool,
    untrusted: bool,
    event: Event,
    #[serde(skip_serializing_if = "Option::is_none")]
    callback_reference: Option<String>,
}
impl ReadEvent {
    pub fn callback_reference(&self) -> Option<&str> {
        self.callback_reference.as_deref()
    }
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }
    pub fn scope(&self) -> ReadScope {
        self.scope
    }
    pub fn event(&self) -> &Event {
        &self.event
    }
    pub fn received_at_ms(&self) -> i64 {
        self.received_at_ms
    }
    pub fn checkpoint_acknowledged(&self) -> bool {
        self.checkpoint_acknowledged
    }
}
#[derive(Serialize)]
pub struct ReadPage {
    schema_version: u32,
    events: Vec<ReadEvent>,
    next_cursor: Cursor,
    has_more: bool,
    #[serde(skip)]
    callbacks: Vec<(usize, CallbackObservation)>,
}
impl ReadPage {
    pub(crate) fn attach_callbacks(
        &mut self,
        mut register: impl FnMut(&CallbackObservation) -> Option<String>,
    ) {
        for (index, observation) in self.callbacks.drain(..) {
            self.events[index].callback_reference = register(&observation);
        }
    }
    pub fn events(&self) -> &[ReadEvent] {
        &self.events
    }
    pub fn next_cursor(&self) -> &Cursor {
        &self.next_cursor
    }
    pub fn has_more(&self) -> bool {
        self.has_more
    }
}
impl std::fmt::Debug for ReadPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoReadPage([REDACTED])")
    }
}
impl std::fmt::Debug for ReadEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoReadEvent([REDACTED])")
    }
}
#[derive(Clone)]
struct Authorization {
    policy: PolicyStore,
    principal: Principal,
    request: Request,
    approval: Option<Approval>,
    expected: ProfileIdentity,
}
impl Authorization {
    fn checked(&self, time: i64) -> AgentResult<(DryRun, Grant)> {
        self.policy.authorized_mtproto(
            &self.principal,
            &self.request,
            self.expected,
            PolicyTime::Current(time),
            self.approval.as_ref(),
            None,
        )
    }
    fn disclose(&self, revision: u64, page: ReadPage) -> AgentResult<ReadPage> {
        let (current, _) = self.checked(time()?)?;
        if current.grant_revision != revision {
            return Err(denied());
        }
        Ok(page)
    }
}
fn denied() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Denied)
}
fn time() -> AgentResult<i64> {
    now().map_err(storage_error)
}

impl Reader {
    pub fn new(feed: Feed, policy: PolicyStore, options: ReadOptions) -> AgentResult<Self> {
        if !(1..=1024).contains(&options.max_in_flight)
            || !(1..=64 * 1024 * 1024).contains(&options.max_read_bytes)
        {
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy));
        }
        Ok(Self {
            feed,
            policy,
            slots: Arc::new(Semaphore::new(options.max_in_flight)),
            options,
        })
    }
    fn local_authorization(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> AgentResult<(Authorization, Option<Cursor>)> {
        let Call::ReadUpdates(args) = request.call() else {
            return Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction));
        };
        if request.profile() != self.feed.profile {
            return Err(denied());
        }
        let auth = Authorization {
            policy: self.policy.clone(),
            principal: principal.clone(),
            request: request.clone(),
            approval: approval.cloned(),
            expected: self.feed.account,
        };
        auth.checked(time()?)?;
        let cursor = args
            .cursor
            .as_deref()
            .map(str::parse::<Cursor>)
            .transpose()
            .map_err(storage_error)?;
        Ok((auth, cursor))
    }
    async fn preflight(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> AgentResult<(Authorization, Option<Cursor>)> {
        let (auth, cursor) = self.local_authorization(principal, request, approval)?;
        self.feed
            .check_cursor_position(cursor.as_ref())
            .await
            .map_err(storage_error)?;
        auth.checked(time()?)?;
        Ok((auth, cursor))
    }
    /// Checks policy and cursor metadata only. No quota charge or payload read.
    pub async fn dry_run(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> AgentResult<DryRun> {
        let (auth, _) = self.preflight(principal, request, approval).await?;
        auth.checked(time()?).map(|(dry, _)| dry)
    }
    /// Explicit host approval for this provider-bound read request.
    pub async fn approve(
        &self,
        principal: &Principal,
        request: &Request,
        expires_at_ms: i64,
    ) -> AgentResult<Approval> {
        // The current grant may require the very approval being issued.
        // approve_mtproto performs the full non-approval policy check itself.
        if !matches!(request.call(), Call::ReadUpdates(_)) {
            return Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction));
        }
        if request.profile() != self.feed.profile {
            return Err(denied());
        }
        let approval = self.policy.approve_mtproto_with_scope(
            principal,
            request,
            self.feed.account,
            PolicyTime::Current(time()?),
            expires_at_ms,
            None,
        )?;
        self.preflight(principal, request, Some(&approval)).await?;
        Ok(approval)
    }
    pub async fn read(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> AgentResult<ReadPage> {
        self.read_with_revision(principal, request, approval)
            .await
            .map(|(_, mut page)| {
                page.callbacks.clear();
                page
            })
    }
    pub(crate) async fn read_with_revision(
        &self,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> AgentResult<(u64, ReadPage)> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::Capacity))?;
        let (auth, cursor) = self.local_authorization(principal, request, approval)?;
        let invocation = auth.clone();
        let Call::ReadUpdates(args) = request.call() else {
            unreachable!("preflight action");
        };
        let limit = args.limit;
        let feed = self.feed.clone();
        let budget = self.options.max_read_bytes;
        // One owned native job retains admission after caller Drop. A committed
        // quota is never rolled back by a later payload/disclosure failure.
        let (revision, page) = self
            .feed
            .journal
            .run(move |c, options, _| {
                let _slot = slot;
                Ok((|| -> AgentResult<(u64, ReadPage)> {
                    invocation.checked(time()?)?;
                    feed.check_cursor_stored(c, cursor.as_ref())
                        .map_err(storage_error)?;
                    charge_invocation_checked(c, options, Some(feed.account), |time| {
                        let (_, grant) = invocation
                            .checked(time)
                            .map_err(|_| JournalError::new(JournalErrorKind::Denied))?;
                        let cost = grant.cost(invocation.request.action());
                        Ok((
                            BudgetContext {
                                profile: invocation.request.profile().into(),
                                principal: invocation.principal.as_str().into(),
                                budget: grant.budget().clone(),
                                cost,
                            },
                            cost,
                        ))
                    })
                    .map_err(storage_error)?;
                    let page = feed
                        .read_stored(c, cursor.as_ref(), 100, budget)
                        .map_err(storage_error)?;
                    let (checked, grant) = invocation.checked(time()?)?;
                    Ok((checked.grant_revision, select(page, &grant, limit)))
                })())
            })
            .await
            .map_err(storage_error)??;
        // A page may wait in the reply channel while the caller is descheduled.
        auth.disclose(revision, page).map(|page| (revision, page))
    }
}
fn select(page: Page, grant: &Grant, limit: u8) -> ReadPage {
    let mut next_cursor = page.next_cursor().clone();
    let has_more = page.has_more();
    let count = page.events().len();
    let mut considered = 0;
    let mut events = Vec::new();
    let mut callbacks = Vec::new();
    for stored in page.into_events() {
        considered += 1;
        next_cursor = stored.cursor.clone();
        let peer = stored.event.peer();
        if peer.map_or(grant.permits_unscoped_updates(), |p| grant.permits_peer(p)) {
            if let Some(observation) = stored.callback_observation() {
                callbacks.push((events.len(), observation));
            }
            events.push(ReadEvent {
                cursor: stored.cursor,
                scope: peer.map_or(ReadScope::Unscoped, ReadScope::Peer),
                received_at_ms: stored.received_at_ms,
                checkpoint_acknowledged: stored.checkpoint_acknowledged,
                untrusted: true,
                event: stored.event,
                callback_reference: None,
            });
            if events.len() == limit as usize {
                break;
            }
        }
    }
    ReadPage {
        schema_version: 1,
        events,
        next_cursor,
        has_more: has_more || considered < count,
        callbacks,
    }
}

#[cfg(test)]
#[path = "reader_tests.rs"]
mod tests;
