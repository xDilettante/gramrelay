//! Owned observable update reception.
mod engine;
mod peers;
#[cfg(test)]
#[path = "updates/source_tests.rs"]
mod source_tests;
use super::*;
pub use crate::session::updates::{GapReason as UpdateGapReason, GapScope as UpdateScope};
use crate::{
    AccountIdentity, SessionError, SessionErrorKind,
    session::updates::{BatchKind, PendingUpdates, UpdateDraft, UpdateGap as StoredGap},
};
use engine::{Machine, Packet, Work};
use grammers_session::{types::UpdatesState, updates::UpdatesLike};
use std::sync::Weak;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateMode {
    Resume,
    StartNow,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateBatchKind {
    Baseline,
    Events,
    Resync,
}
#[derive(Clone)]
struct Binding {
    owner: Weak<Core>,
    expected: AccountIdentity,
    epoch: u64,
    generation: Arc<AtomicU64>,
}
impl Binding {
    fn current(&self) -> Result<()> {
        if self.owner.strong_count() == 0 || self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(invalid());
        }
        Ok(())
    }
    fn check(&self, other: &Self) -> Result<()> {
        self.current()?;
        other.current()?;
        if !Weak::ptr_eq(&self.owner, &other.owner)
            || self.expected != other.expected
            || self.epoch != other.epoch
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub struct UpdateBatch {
    binding: Binding,
    pending: PendingUpdates,
    packet: Packet,
}
impl fmt::Debug for UpdateBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateBatch")
            .field("kind", &self.kind())
            .field("events", &self.packet.events.len())
            .finish_non_exhaustive()
    }
}
impl UpdateBatch {
    /// Stable non-authorizing deduplication metadata, including after restart.
    pub fn id(&self) -> [u8; 32] {
        self.pending.id.fingerprint()
    }
    pub fn kind(&self) -> UpdateBatchKind {
        match self.pending.draft.kind {
            BatchKind::Baseline => UpdateBatchKind::Baseline,
            BatchKind::Events => UpdateBatchKind::Events,
            BatchKind::Resync { .. } => UpdateBatchKind::Resync,
        }
    }
    /// Trusted Rust content access; never expose raw peer/file keys to agent JSON.
    pub fn updates(&self) -> &[tl::enums::Update] {
        &self.packet.events
    }
    pub fn users(&self) -> &[tl::enums::User] {
        &self.packet.users
    }
    pub fn chats(&self) -> &[tl::enums::Chat] {
        &self.packet.chats
    }
}
pub struct ResyncRequired {
    binding: Binding,
    gap: StoredGap,
    restart: bool,
}
impl fmt::Debug for ResyncRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResyncRequired")
            .field("scope", &self.scope())
            .field("reason", &self.reason())
            .field("restart_required", &self.restart)
            .finish_non_exhaustive()
    }
}
impl ResyncRequired {
    pub fn id(&self) -> [u8; 32] {
        self.gap.id.fingerprint()
    }
    pub fn scope(&self) -> UpdateScope {
        self.gap.scope
    }
    pub fn reason(&self) -> UpdateGapReason {
        self.gap.reason
    }
    pub fn restart_required(&self) -> bool {
        self.restart
    }
}
#[derive(Debug)]
pub enum UpdateItem {
    Batch(UpdateBatch),
    Gap(ResyncRequired),
}
pub struct UpdateReceiver {
    account: Account,
    state: Arc<ReceiverState>,
    hub: Arc<Hub>,
}
impl fmt::Debug for UpdateReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UpdateReceiver([REDACTED])")
    }
}
struct ReceiverState {
    binding: Binding,
    mode: UpdateMode,
    cursor: tokio::sync::Mutex<Cursor>,
}
#[derive(Default)]
struct Cursor {
    machine: Option<Machine>,
    loss_recorded: bool,
}
#[derive(Default)]
pub(super) struct Hub {
    source: tokio::sync::Mutex<Option<Source>>,
    claim: Mutex<Weak<ReceiverState>>,
}
struct Source {
    updates: mpsc::Receiver<UpdatesLike>,
    loss: watch::Receiver<bool>,
}
enum Received {
    Update(Box<UpdatesLike>),
    Loss,
    Idle,
}
impl Source {
    async fn receive(&mut self, wake: tokio::time::Instant) -> Result<Received> {
        tokio::select! {biased;
            result=self.loss.wait_for(|lost|*lost)=>match result {Ok(_)=>Ok(Received::Loss),Err(_)=>Err(error(ClientErrorKind::Backend,Outcome::Unknown))},
            input=self.updates.recv()=>input.map(|input|Received::Update(Box::new(input))).ok_or_else(||error(ClientErrorKind::Backend,Outcome::Unknown)),
            _=tokio::time::sleep_until(wake)=>Ok(Received::Idle),
        }
    }
}
impl Hub {
    pub(super) async fn close(&self) {
        self.source.lock().await.take();
    }
    pub(super) async fn install(
        &self,
        updates: mpsc::Receiver<UpdatesLike>,
        loss: watch::Receiver<bool>,
    ) {
        *self.source.lock().await = Some(Source { updates, loss });
    }
    async fn lost(&self) -> bool {
        self.source
            .lock()
            .await
            .as_ref()
            .is_some_and(|source| *source.loss.borrow())
    }
}
fn invalid() -> ClientError {
    error(ClientErrorKind::InvalidUpdate, Outcome::NotSent)
}
fn stored(failure: SessionError) -> ClientError {
    match failure.kind() {
        SessionErrorKind::InvalidInput => invalid(),
        SessionErrorKind::Quarantined => error(ClientErrorKind::Quarantined, Outcome::NotSent),
        _ => error(ClientErrorKind::Session, Outcome::Unknown),
    }
}
async fn invoke<R: tl::RemoteCall>(
    context: &runtime::Context,
    request: &R,
    limit: usize,
) -> Result<R::Return>
where
    R::Return: Send,
{
    let mut bytes = encode_request(request, limit)?;
    context.invoke::<R>(std::mem::take(&mut *bytes)).await
}
impl UpdateReceiver {
    pub(super) fn open(
        client: Client,
        expected: AccountIdentity,
        mode: UpdateMode,
    ) -> Result<Self> {
        let hub = client.inner.updates.clone();
        let state = Arc::new(ReceiverState {
            binding: Binding {
                owner: Arc::downgrade(&client.inner),
                expected,
                epoch: client.inner.auth_epoch.load(Ordering::Acquire),
                generation: client.inner.auth_epoch.clone(),
            },
            mode,
            cursor: Default::default(),
        });
        {
            let mut claim = hub
                .claim
                .lock()
                .map_err(|_| error(ClientErrorKind::Backend, Outcome::NotSent))?;
            if claim.upgrade().is_some() {
                return Err(error(
                    ClientErrorKind::UpdateReceiverOwned,
                    Outcome::NotSent,
                ));
            }
            *claim = Arc::downgrade(&state);
        }
        Ok(Self {
            account: client.account(expected),
            state,
            hub,
        })
    }
    /// Wait at most `wait`, possibly less to retain room in the owned deadline.
    /// Pending durable data is replayed before another update/difference request.
    pub async fn next_batch(&mut self, wait: Duration) -> Result<Option<UpdateItem>> {
        if wait > Duration::from_secs(300) {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        self.state.binding.current()?;
        let state = self.state.clone();
        let hub = self.hub.clone();
        let limit = self.account.request_limit();
        self.account
            .run(move |context, session| async move {
                state.binding.current()?;
                let mut cursor = state.cursor.lock().await;
                observe_loss(&hub, &mut cursor, &session, state.binding.expected).await?;
                let view = session
                    .updates_view(state.binding.expected)
                    .map_err(stored)?;
                if let Some(pending) = view.pending {
                    return make_batch(&context, state.binding.clone(), pending)
                        .await
                        .map(|batch| Some(UpdateItem::Batch(batch)));
                }
                if let Some(gap) = view.gap {
                    return Ok(Some(UpdateItem::Gap(ResyncRequired {
                        binding: state.binding.clone(),
                        gap,
                        restart: hub.lost().await,
                    })));
                }
                let Some(base) = view.acknowledged else {
                    if state.mode == UpdateMode::Resume {
                        return gap(
                            &context,
                            &session,
                            &state.binding,
                            &mut cursor,
                            &hub,
                            (UpdateScope::Account, UpdateGapReason::MissingBaseline),
                        )
                        .await
                        .map(Some);
                    }
                    let tl::enums::updates::State::State(current) =
                        invoke(&context, &tl::functions::updates::GetState {}, limit).await?;
                    let checkpoint = UpdatesState {
                        pts: current.pts,
                        qts: current.qts,
                        date: current.date,
                        seq: current.seq,
                        channels: vec![],
                    };
                    let checked = context
                        .compute(move || Ok(engine::checked_state(checkpoint)))
                        .await?;
                    let checkpoint = match checked {
                        Ok(state) => state,
                        Err(reason) => {
                            return gap(
                                &context,
                                &session,
                                &state.binding,
                                &mut cursor,
                                &hub,
                                reason,
                            )
                            .await
                            .map(Some);
                        }
                    };
                    let pending = session
                        .stage_updates(UpdateDraft {
                            account: state.binding.expected,
                            kind: BatchKind::Baseline,
                            base: None,
                            checkpoint,
                            payload: vec![].into(),
                        })
                        .await
                        .map_err(stored)?;
                    return make_batch(&context, state.binding.clone(), pending)
                        .await
                        .map(|batch| Some(UpdateItem::Batch(batch)));
                };
                let mut machine = if let Some(machine) = cursor.machine.take() {
                    machine
                } else {
                    let initial = context
                        .compute(move || Ok(Machine::load(base.clone())))
                        .await?;
                    match initial {
                        Ok(machine) => machine,
                        Err(reason) => {
                            return gap(
                                &context,
                                &session,
                                &state.binding,
                                &mut cursor,
                                &hub,
                                reason,
                            )
                            .await
                            .map(Some);
                        }
                    }
                };
                let until = context.polling_deadline(wait);
                let mut steps = 0;
                loop {
                    if steps == 64 {
                        cursor.machine = Some(machine);
                        return Ok(None);
                    }
                    steps += 1;
                    context.check_active()?;
                    if hub.lost().await {
                        return gap(
                            &context,
                            &session,
                            &state.binding,
                            &mut cursor,
                            &hub,
                            (UpdateScope::Account, UpdateGapReason::QueueOverflow),
                        )
                        .await
                        .map(Some);
                    }
                    let (returned, work) = context
                        .compute(move || {
                            let work = machine.work();
                            Ok((machine, work))
                        })
                        .await?;
                    machine = returned;
                    let before = machine.state();
                    let applied = match work {
                        Work::Difference(request) => {
                            let response = match invoke(&context, &request, limit).await {
                                Err(failure)
                                    if failure.rpc_class()
                                        == super::error::RpcClass::TimestampUnavailable =>
                                {
                                    return gap(
                                        &context,
                                        &session,
                                        &state.binding,
                                        &mut cursor,
                                        &hub,
                                        (UpdateScope::Account, UpdateGapReason::MissingBaseline),
                                    )
                                    .await
                                    .map(Some);
                                }
                                result => result?,
                            };
                            peers::apply(
                                &context,
                                &session,
                                state.binding.expected,
                                machine,
                                response,
                                Machine::difference,
                            )
                            .await?
                        }
                        Work::Channel(mut request) => {
                            let tl::enums::InputChannel::Channel(channel) = &request.channel else {
                                return Err(invalid());
                            };
                            let id = channel.channel_id;
                            let peer = session
                                .cached_peer(
                                    grammers_session::types::PeerId::channel(id)
                                        .ok_or_else(invalid)?,
                                )
                                .await
                                .map_err(stored)?;
                            let Some(grammers_session::types::PeerInfo::Channel {
                                auth: Some(auth),
                                ..
                            }) = peer
                            else {
                                return gap(
                                    &context,
                                    &session,
                                    &state.binding,
                                    &mut cursor,
                                    &hub,
                                    (UpdateScope::Channel(id), UpdateGapReason::MissingHash),
                                )
                                .await
                                .map(Some);
                            };
                            request.channel = tl::types::InputChannel {
                                channel_id: id,
                                access_hash: auth.hash(),
                            }
                            .into();
                            request.limit = 100;
                            let response = match invoke(&context, &request, limit).await {
                                Err(failure)
                                    if failure.rpc_class()
                                        == super::error::RpcClass::TimestampUnavailable =>
                                {
                                    return gap(
                                        &context,
                                        &session,
                                        &state.binding,
                                        &mut cursor,
                                        &hub,
                                        (
                                            UpdateScope::Channel(id),
                                            UpdateGapReason::MissingBaseline,
                                        ),
                                    )
                                    .await
                                    .map(Some);
                                }
                                Err(failure)
                                    if failure.rpc_class()
                                        == super::error::RpcClass::ChannelAccessLost =>
                                {
                                    return gap(
                                        &context,
                                        &session,
                                        &state.binding,
                                        &mut cursor,
                                        &hub,
                                        (
                                            UpdateScope::Channel(id),
                                            UpdateGapReason::ChannelAccessLost,
                                        ),
                                    )
                                    .await
                                    .map(Some);
                                }
                                result => result?,
                            };
                            peers::apply(
                                &context,
                                &session,
                                state.binding.expected,
                                machine,
                                response,
                                move |machine, response| machine.channel(id, response),
                            )
                            .await?
                        }
                        Work::Socket(deadline) => {
                            let mut source_guard = hub.source.lock().await;
                            let source = source_guard
                                .as_mut()
                                .ok_or_else(|| error(ClientErrorKind::Backend, Outcome::Unknown))?;
                            let wake = tokio::time::Instant::from_std(deadline).min(until);
                            let received = source.receive(wake).await?;
                            drop(source_guard);
                            let input = match received {
                                Received::Update(input) => *input,
                                Received::Loss => {
                                    return gap(
                                        &context,
                                        &session,
                                        &state.binding,
                                        &mut cursor,
                                        &hub,
                                        (UpdateScope::Account, UpdateGapReason::QueueOverflow),
                                    )
                                    .await
                                    .map(Some);
                                }
                                Received::Idle => {
                                    if tokio::time::Instant::now() >= until {
                                        cursor.machine = Some(machine);
                                        return Ok(None);
                                    }
                                    continue;
                                }
                            };
                            peers::apply(
                                &context,
                                &session,
                                state.binding.expected,
                                machine,
                                input,
                                Machine::socket,
                            )
                            .await?
                        }
                    };
                    machine = applied.0;
                    let packet = match applied.1 {
                        Ok(packet) => packet,
                        Err(reason) => {
                            return gap(
                                &context,
                                &session,
                                &state.binding,
                                &mut cursor,
                                &hub,
                                reason,
                            )
                            .await
                            .map(Some);
                        }
                    };
                    let checkpoint = machine.state();
                    if packet.events.is_empty()
                        && packet.users.is_empty()
                        && packet.chats.is_empty()
                        && checkpoint == before
                    {
                        if tokio::time::Instant::now() >= until {
                            cursor.machine = Some(machine);
                            return Ok(None);
                        }
                        continue;
                    }
                    let expected = state.binding.expected;
                    let prepared = context.compute(move || Ok(packet.encode())).await?;
                    let body = match prepared {
                        Ok(result) => result,
                        Err(reason) => {
                            return gap(
                                &context,
                                &session,
                                &state.binding,
                                &mut cursor,
                                &hub,
                                reason,
                            )
                            .await
                            .map(Some);
                        }
                    };
                    context.check_active()?;
                    if hub.lost().await {
                        return gap(
                            &context,
                            &session,
                            &state.binding,
                            &mut cursor,
                            &hub,
                            (UpdateScope::Account, UpdateGapReason::QueueOverflow),
                        )
                        .await
                        .map(Some);
                    }
                    let base = session.updates_view(expected).map_err(stored)?.acknowledged;
                    let pending = session
                        .stage_updates(UpdateDraft {
                            account: expected,
                            kind: BatchKind::Events,
                            base,
                            checkpoint,
                            payload: body.to_vec().into(),
                        })
                        .await
                        .map_err(stored)?;
                    cursor.machine = Some(machine);
                    return make_batch(&context, state.binding.clone(), pending)
                        .await
                        .map(|batch| Some(UpdateItem::Batch(batch)));
                }
            })
            .await
    }
    pub async fn acknowledge(&mut self, batch: &UpdateBatch) -> Result<()> {
        self.state.binding.check(&batch.binding)?;
        let id = batch.pending.id;
        let state = self.state.clone();
        let hub = self.hub.clone();
        self.account
            .run(move |context, session| async move {
                state.binding.current()?;
                let mut cursor = state.cursor.lock().await;
                observe_loss(&hub, &mut cursor, &session, state.binding.expected).await?;
                context.check_active()?;
                session
                    .acknowledge_updates(state.binding.expected, id)
                    .await
                    .map_err(stored)
            })
            .await
    }
    /// Explicitly accept skipped history or completed host reconstruction.
    /// The new checkpoint is an empty decision batch and still needs ACK.
    /// On a channel gap only that channel advances; other counters are preserved.
    pub async fn resync_from_current(&mut self, required: &ResyncRequired) -> Result<UpdateBatch> {
        self.state.binding.check(&required.binding)?;
        let id = required.gap.id;
        let state = self.state.clone();
        let hub = self.hub.clone();
        let limit = self.account.request_limit();
        self.account
            .run(move |context, session| async move {
                state.binding.current()?;
                let mut cursor = state.cursor.lock().await;
                observe_loss(&hub, &mut cursor, &session, state.binding.expected).await?;
                if hub.lost().await {
                    return Err(error(ClientErrorKind::RestartRequired, Outcome::NotSent));
                }
                let view = session
                    .updates_view(state.binding.expected)
                    .map_err(stored)?;
                if view.pending.is_some() {
                    return Err(invalid());
                }
                let current = view.gap.filter(|gap| gap.id == id).ok_or_else(invalid)?;
                let base = view.acknowledged;
                let mut checkpoint = base.clone().unwrap_or_default();
                match current.scope {
                    UpdateScope::Account => {
                        let tl::enums::updates::State::State(current) =
                            invoke(&context, &tl::functions::updates::GetState {}, limit).await?;
                        checkpoint.pts = current.pts;
                        checkpoint.qts = current.qts;
                        checkpoint.seq = current.seq;
                        checkpoint.date = current.date;
                    }
                    UpdateScope::Channel(channel_id) => {
                        if base.is_none() {
                            return Err(invalid());
                        }
                        let backend = grammers_session::types::PeerId::channel(channel_id)
                            .ok_or_else(invalid)?;
                        let Some(grammers_session::types::PeerInfo::Channel {
                            auth: Some(auth),
                            ..
                        }) = session.cached_peer(backend).await.map_err(stored)?
                        else {
                            return Err(error(ClientErrorKind::PeerUnavailable, Outcome::NotSent));
                        };
                        let reply = invoke(
                            &context,
                            &tl::functions::channels::GetFullChannel {
                                channel: tl::types::InputChannel {
                                    channel_id,
                                    access_hash: auth.hash(),
                                }
                                .into(),
                            },
                            limit,
                        )
                        .await;
                        let full = match reply {
                            Err(failure)
                                if failure.rpc_class()
                                    == super::error::RpcClass::ChannelAccessLost =>
                            {
                                session
                                    .record_update_gap(
                                        state.binding.expected,
                                        UpdateScope::Channel(channel_id),
                                        UpdateGapReason::ChannelAccessLost,
                                    )
                                    .await
                                    .map_err(stored)?;
                                cursor.machine = None;
                                return Err(failure);
                            }
                            result => result?,
                        };
                        let tl::enums::messages::ChatFull::Full(full) = full;
                        let tl::enums::ChatFull::ChannelFull(channel) = full.full_chat else {
                            return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
                        };
                        if channel.id != channel_id {
                            return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
                        }
                        let expected = state.binding.expected;
                        let peers = context
                            .compute(move || {
                                common::checked_entities(full.users, full.chats, expected)
                            })
                            .await?;
                        if !peers.is_empty() {
                            session.cache_peers(peers).await.map_err(stored)?;
                        }
                        if let Some(known) = checkpoint
                            .channels
                            .iter_mut()
                            .find(|known| known.id == channel_id)
                        {
                            known.pts = channel.pts;
                        } else {
                            checkpoint
                                .channels
                                .push(grammers_session::types::ChannelState {
                                    id: channel_id,
                                    pts: channel.pts,
                                });
                        }
                    }
                }
                let checked = context
                    .compute(move || Ok(engine::checked_state(checkpoint)))
                    .await?;
                let checkpoint =
                    checked.map_err(|_| error(ClientErrorKind::InvalidUpdate, Outcome::NotSent))?;
                context.check_active()?;
                let pending = session
                    .stage_updates(UpdateDraft {
                        account: state.binding.expected,
                        kind: BatchKind::Resync { gap: id },
                        base,
                        checkpoint,
                        payload: vec![].into(),
                    })
                    .await
                    .map_err(stored)?;
                cursor.machine = None;
                make_batch(&context, state.binding.clone(), pending).await
            })
            .await
    }
    /// Remove an inaccessible channel from the current recovery checkpoints.
    /// Later channel events require a new explicit baseline. ACK is mandatory.
    pub async fn retire_channel(&mut self, required: &ResyncRequired) -> Result<UpdateBatch> {
        self.state.binding.check(&required.binding)?;
        if required.reason() != UpdateGapReason::ChannelAccessLost {
            return Err(invalid());
        }
        let UpdateScope::Channel(channel) = required.scope() else {
            return Err(invalid());
        };
        let id = required.gap.id;
        let state = self.state.clone();
        let hub = self.hub.clone();
        self.account
            .run(move |context, session| async move {
                state.binding.current()?;
                let mut cursor = state.cursor.lock().await;
                observe_loss(&hub, &mut cursor, &session, state.binding.expected).await?;
                if hub.lost().await {
                    return Err(error(ClientErrorKind::RestartRequired, Outcome::NotSent));
                }
                let view = session
                    .updates_view(state.binding.expected)
                    .map_err(stored)?;
                if view.pending.is_some() || view.gap.is_none_or(|gap| gap.id != id) {
                    return Err(invalid());
                }
                let base = view.acknowledged;
                let mut checkpoint = base.clone().ok_or_else(invalid)?;
                checkpoint.channels.retain(|known| known.id != channel);
                let pending = session
                    .stage_updates(UpdateDraft {
                        account: state.binding.expected,
                        kind: BatchKind::Resync { gap: id },
                        base,
                        checkpoint,
                        payload: vec![].into(),
                    })
                    .await
                    .map_err(stored)?;
                cursor.machine = None;
                make_batch(&context, state.binding.clone(), pending).await
            })
            .await
    }
}
async fn observe_loss(
    hub: &Hub,
    cursor: &mut Cursor,
    session: &Session,
    expected: AccountIdentity,
) -> Result<()> {
    if !cursor.loss_recorded && hub.lost().await {
        session
            .record_update_gap(
                expected,
                UpdateScope::Account,
                UpdateGapReason::QueueOverflow,
            )
            .await
            .map_err(stored)?;
        cursor.loss_recorded = true;
        cursor.machine = None;
    }
    Ok(())
}
async fn make_batch(
    context: &runtime::Context,
    binding: Binding,
    pending: PendingUpdates,
) -> Result<UpdateBatch> {
    context
        .compute(move || {
            let packet = Packet::decode(&pending.draft.payload)
                .map_err(|_| error(ClientErrorKind::Decode, Outcome::Unknown))?;
            Ok(UpdateBatch {
                binding,
                pending,
                packet,
            })
        })
        .await
}
async fn gap(
    context: &runtime::Context,
    session: &Session,
    binding: &Binding,
    cursor: &mut Cursor,
    hub: &Hub,
    reason: engine::Gap,
) -> Result<UpdateItem> {
    context.check_active()?;
    let gap = session
        .record_update_gap(binding.expected, reason.0, reason.1)
        .await
        .map_err(stored)?;
    cursor.machine = None;
    if reason.1 == UpdateGapReason::QueueOverflow {
        cursor.loss_recorded = true;
    }
    Ok(UpdateItem::Gap(ResyncRequired {
        binding: binding.clone(),
        gap,
        restart: hub.lost().await,
    }))
}
