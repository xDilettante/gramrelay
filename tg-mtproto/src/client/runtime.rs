use super::*;
use crate::session::Health;
use grammers_mtsender::SenderPoolFatHandle;
use std::collections::HashMap;
use tl::Deserializable;
use tokio::task::JoinSet;

#[derive(Clone)]
pub(super) struct Context {
    handle: SenderPoolFatHandle,
    session: Session,
    max_response_bytes: usize,
    state: Arc<CallState>,
    shared: Arc<Shared>,
    deadline: tokio::time::Instant,
    blocking: Arc<tokio::sync::Mutex<JoinSet<()>>>,
    #[cfg(test)]
    before_compute: Option<Duration>,
    #[cfg(test)]
    before_dc_confirm: Option<Duration>,
}
impl Context {
    pub(super) fn polling_deadline(&self, wait: Duration) -> tokio::time::Instant {
        let now = tokio::time::Instant::now();
        now + wait.min(self.deadline.saturating_duration_since(now) / 2)
    }
    pub async fn invoke_auth<R: tl::RemoteCall>(&self, body: &[u8]) -> Result<R::Return>
    where
        R::Return: Send,
    {
        loop {
            match self.invoke::<R>(body.to_vec()).await {
                Err(failure) => {
                    let super::error::RpcClass::Migrate(dc) = failure.rpc_class() else {
                        return Err(failure);
                    };
                    self.check_active()?;
                    let old_dc = self
                        .handle
                        .session
                        .home_dc_id()
                        .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?;
                    if dc == old_dc
                        || !self
                            .session
                            .has_datacenter(dc)
                            .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?
                    {
                        return Err(failure);
                    }
                    if self.state.migration_used.swap(true, Ordering::AcqRel) {
                        return Err(failure);
                    }
                    self.handle
                        .session
                        .set_home_dc_id(dc)
                        .await
                        .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?;
                    self.handle.disconnect_from_dc(old_dc);
                }
                result => return result,
            }
        }
    }
    pub(super) fn check_active(&self) -> Result<()> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(error(ClientErrorKind::Timeout, self.state.seal()));
        }
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(error(ClientErrorKind::Closed, self.state.seal()));
        }
        if self
            .session
            .is_quarantined()
            .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?
        {
            return Err(error(ClientErrorKind::Quarantined, self.state.seal()));
        }
        Ok(())
    }
    pub async fn compute<T, F>(&self, function: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        self.check_active()?;
        let (reply, waiting) = oneshot::channel();
        {
            let mut jobs = self.blocking.lock().await;
            // Serialize computation and reap its tail before admitting another.
            while let Some(result) = jobs.join_next().await {
                if result.is_err_and(|error| error.is_panic()) {
                    return Err(error(ClientErrorKind::Backend, self.state.seal()));
                }
            }
            self.check_active()?;
            #[cfg(test)]
            let before_compute = self.before_compute;
            jobs.spawn_blocking(move || {
                #[cfg(test)]
                if let Some(delay) = before_compute {
                    std::thread::sleep(delay);
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(function))
                    .unwrap_or_else(|_| Err(error(ClientErrorKind::Decode, Outcome::Unknown)));
                let _ = reply.send(result);
            });
        }
        waiting
            .await
            .map_err(|_| error(ClientErrorKind::Backend, self.state.seal()))?
    }
    pub fn stop_after_action(&self) {
        self.state.stop_generation.store(true, Ordering::Release);
    }
    pub fn confirm_logout(&self) {
        self.state.logout_confirmed.store(true, Ordering::Release);
    }
    // The job state, rather than the action future, owns admission guards through
    // final outcome classification. Releasing a writer can wake another thread.
    pub async fn hold_authorization_read(&self, gate: Arc<tokio::sync::RwLock<()>>) {
        let guard = AuthorizationGuard::Read {
            _guard: gate.read_owned().await,
        };
        *self
            .state
            .authorization
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(guard);
    }
    pub async fn hold_authorization_write(&self, gate: Arc<tokio::sync::RwLock<()>>) {
        let guard = AuthorizationGuard::Write {
            _guard: gate.write_owned().await,
        };
        *self
            .state
            .authorization
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(guard);
    }
    pub async fn invoke<R: tl::RemoteCall>(&self, body: Vec<u8>) -> Result<R::Return> {
        self.invoke_inner::<R>(body, false, None).await
    }
    pub fn home_dc(&self) -> Result<i32> {
        self.handle
            .session
            .home_dc_id()
            .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))
    }
    pub async fn invoke_in_dc<R: tl::RemoteCall>(
        &self,
        dc: i32,
        body: Vec<u8>,
    ) -> Result<R::Return> {
        self.invoke_inner::<R>(body, false, Some(dc)).await
    }
    pub fn begin_dc_authorization(&self, guard: tokio::sync::OwnedMutexGuard<()>) -> Result<()> {
        self.check_active()?;
        let mut held = self
            .state
            .dc_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.is_some() {
            return Err(error(ClientErrorKind::Backend, self.state.seal()));
        }
        *held = Some(guard);
        self.state.dc_pending.store(true, Ordering::Release);
        Ok(())
    }
    pub fn confirm_dc_authorization(&self) -> Result<()> {
        let guard = {
            let completion = self
                .state
                .completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *completion != Completion::Pending {
                return Err(error(ClientErrorKind::Closed, self.state.seal()));
            }
            self.check_active()?;
            self.session
                .live()
                .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?;
            #[cfg(test)]
            if let Some(delay) = self.before_dc_confirm {
                std::thread::sleep(delay);
            }
            let mut held = self
                .state
                .dc_guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if held.is_none() {
                return Err(error(ClientErrorKind::Backend, self.state.seal()));
            }
            // Session/guard reads above may block. These must be the final
            // nonblocking checks before publication and waking DC waiters.
            if tokio::time::Instant::now() >= self.deadline {
                return Err(error(ClientErrorKind::Timeout, self.state.seal()));
            }
            if self.shared.closed.load(Ordering::Acquire) {
                return Err(error(ClientErrorKind::Closed, self.state.seal()));
            }
            let guard = held
                .take()
                .ok_or_else(|| error(ClientErrorKind::Backend, self.state.seal()))?;
            self.state.dc_pending.store(false, Ordering::Release);
            guard
        };
        // No waiter is woken under the completion mutex.
        drop(guard);
        Ok(())
    }
    pub async fn invoke_logout(&self, body: Vec<u8>) -> Result<tl::enums::auth::LoggedOut> {
        self.invoke_inner::<tl::functions::auth::LogOut>(body, true, None)
            .await
    }
    async fn invoke_inner<R: tl::RemoteCall>(
        &self,
        body: Vec<u8>,
        allow_quarantine: bool,
        target_dc: Option<i32>,
    ) -> Result<R::Return> {
        let mut body = Zeroizing::new(body);
        if self
            .session
            .is_quarantined()
            .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?
            && !allow_quarantine
        {
            return Err(error(ClientErrorKind::Quarantined, self.state.seal()));
        }
        let dc = match target_dc {
            Some(dc) => {
                if dc <= 0
                    || !self
                        .session
                        .has_datacenter(dc)
                        .map_err(|_| error(ClientErrorKind::Session, self.state.seal()))?
                {
                    return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
                }
                dc
            }
            None => self.home_dc()?,
        };
        if tokio::time::Instant::now() >= self.deadline {
            return Err(error(ClientErrorKind::Timeout, self.state.seal()));
        }
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(error(ClientErrorKind::Closed, self.state.seal()));
        }
        if self
            .state
            .phase
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            == Err(2)
        {
            return Err(error(ClientErrorKind::Closed, Outcome::NotSent));
        }
        let bytes = Zeroizing::new(
            self.handle
                .raw_invoke_in_dc(dc, std::mem::take(&mut *body))
                .await
                .map_err(super::error::invocation)?,
        );
        self.handle
            .session
            .home_dc_id()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
        if bytes.len() > self.max_response_bytes {
            return Err(error(ClientErrorKind::Capacity, Outcome::Unknown));
        }
        let mut cursor = tl::deserialize::Cursor::from_slice(&bytes);
        let value = R::Return::deserialize(&mut cursor)
            .map_err(|_| error(ClientErrorKind::Decode, Outcome::Unknown))?;
        if cursor.pos() != bytes.len() {
            return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
        }
        Ok(value)
    }
}
type Reply = Arc<Mutex<Option<oneshot::Sender<Result<Value>>>>>;
type Slot = Arc<Mutex<Option<OwnedSemaphorePermit>>>;
enum AuthorizationGuard {
    Read {
        _guard: tokio::sync::OwnedRwLockReadGuard<()>,
    },
    Write {
        _guard: tokio::sync::OwnedRwLockWriteGuard<()>,
    },
}
#[derive(Default, Eq, PartialEq)]
enum Completion {
    #[default]
    Pending,
    Accepted,
    Sealed,
}
#[derive(Default)]
struct CallState {
    // Queue admission and sealing share one linearization point. Once backend
    // admission is attempted, only conservative outcomes may be reported.
    phase: std::sync::atomic::AtomicU8,
    done: AtomicBool,
    authorization: Mutex<Option<AuthorizationGuard>>,
    stop_generation: AtomicBool,
    logout_confirmed: AtomicBool,
    completion: Mutex<Completion>,
    migration_used: AtomicBool,
    dc_pending: AtomicBool,
    dc_guard: Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>,
}
impl CallState {
    fn is_pending(&self) -> bool {
        *self
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == Completion::Pending
    }
    fn seal_completion(&self) -> Option<Outcome> {
        let mut completion = self
            .completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *completion == Completion::Accepted {
            return None;
        }
        *completion = Completion::Sealed;
        self.done.store(true, Ordering::Release);
        Some(self.seal())
    }
    fn authorization_transition(&self) -> bool {
        self.dc_pending.load(Ordering::Acquire)
            || matches!(
                self.authorization
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref(),
                Some(AuthorizationGuard::Write { .. })
            )
    }
    fn release_authorization(&self) {
        let guard = self
            .authorization
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(guard);
        let dc_guard = self
            .dc_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(dc_guard);
    }
    fn seal(&self) -> Outcome {
        match self
            .phase
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(2) => Outcome::NotSent,
            Err(_) => Outcome::Unknown,
        }
    }
}
struct Pending {
    reply: Reply,
    _slot: Slot,
    deadline: tokio::time::Instant,
    state: Arc<CallState>,
}

pub(super) struct ExitGuard {
    shared: Arc<Shared>,
    slots: Arc<Semaphore>,
    finished: bool,
}
impl ExitGuard {
    pub fn new(shared: Arc<Shared>, slots: Arc<Semaphore>) -> Self {
        Self {
            shared,
            slots,
            finished: false,
        }
    }
    fn complete(&mut self) {
        self.finished = true;
    }
}
impl Drop for ExitGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.shared.closed.store(true, Ordering::Release);
            self.slots.close();
            self.shared.stop.send_replace(true);
            self.shared
                .exited
                .send_replace(Some(Err(error(ClientErrorKind::Backend, Outcome::Unknown))));
        }
    }
}
struct StopDriver(grammers_mtsender::SenderPoolHandle);
impl Drop for StopDriver {
    fn drop(&mut self) {
        self.0.quit();
    }
}

fn reply_take(reply: &Reply) -> Option<oneshot::Sender<Result<Value>>> {
    reply
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

pub(super) async fn run(
    mut startup: Startup,
    options: ClientOptions,
    slots: Arc<Semaphore>,
    shared: Arc<Shared>,
    mut exit: ExitGuard,
) {
    let mut stop = shared.stop.subscribe();
    let stopping = *stop.borrow();
    if stopping || startup.claim.live().is_err() {
        shared.closed.store(true, Ordering::Release);
        slots.close();
        startup.receiver.close();
        while let Ok(call) = startup.receiver.try_recv() {
            let _ = call
                .reply
                .send(Err(error(ClientErrorKind::Closed, Outcome::NotSent)));
        }
        let result = startup
            .claim
            .finish()
            .await
            .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown));
        shared.exited.send_replace(Some(result));
        exit.complete();
        return;
    }
    let pool = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (startup.factory)(startup.claim.backend(), startup.claim.api_id())
    }));
    let pool = match pool {
        Ok(pool) => pool,
        Err(_) => {
            shared.closed.store(true, Ordering::Release);
            slots.close();
            startup.receiver.close();
            while let Ok(call) = startup.receiver.try_recv() {
                let _ = call
                    .reply
                    .send(Err(error(ClientErrorKind::Backend, Outcome::NotSent)));
            }
            let _ = startup.claim.finish().await;
            shared
                .exited
                .send_replace(Some(Err(error(ClientErrorKind::Backend, Outcome::NotSent))));
            exit.complete();
            return;
        }
    };
    let _stop_driver = StopDriver(pool.handle.thin.clone());
    let mut driver = tokio::spawn(pool.runner.run());
    let handle = pool.handle;
    startup
        .updates
        .install(pool.updates, handle.thin.update_loss())
        .await;
    let mut health = startup.claim.health();
    let mut jobs = JoinSet::new();
    let blocking = Arc::new(tokio::sync::Mutex::new(JoinSet::<()>::new()));
    let mut active = HashMap::<u64, Pending>::new();
    let mut next_id = 0u64;
    let mut driver_exit = None;
    let reason = loop {
        if shared.closed.load(Ordering::Acquire) {
            break ClientErrorKind::Closed;
        }
        let lease = match startup.claim.deadline() {
            Ok(deadline) => tokio::time::Instant::from_std(deadline),
            Err(_) => break ClientErrorKind::Session,
        };
        let deadline = active
            .values()
            .filter(|pending| !pending.state.done.load(Ordering::Acquire))
            .map(|pending| pending.deadline)
            .min();
        tokio::select! { biased;
            _=async {let _=stop.wait_for(|stop|*stop).await;} =>break ClientErrorKind::Closed,
            changed=health.changed()=> {
                if changed.is_err() || *health.borrow_and_update()!=Health::Ready {break ClientErrorKind::Session;}
            }
            _=tokio::time::sleep_until(lease)=> {if startup.claim.live().is_err() {break ClientErrorKind::Session;}}
            _=async {match deadline {Some(deadline)=>tokio::time::sleep_until(deadline).await,None=>std::future::pending().await}}=> {
                let now=tokio::time::Instant::now();
                if active.values().any(|pending|!pending.state.done.load(Ordering::Acquire) && pending.deadline<=now && pending.state.is_pending()) {break ClientErrorKind::Timeout;}
            }
            result=&mut driver=> {driver_exit=Some(result);break ClientErrorKind::Backend;}
            completed=jobs.join_next(),if !jobs.is_empty()=> {
                match completed {
                    Some(Ok((id,fence)))=> {
                        if let Some(reason)=fence {break reason;}
                        if active.remove(&id).is_none() {break ClientErrorKind::Backend;}
                    }
                    _=>break ClientErrorKind::Backend,
                }
            }
            call=startup.receiver.recv()=> {
                let Some(call)=call else {break ClientErrorKind::Closed;};
                if shared.closed.load(Ordering::Acquire) {let _=call.reply.send(Err(error(ClientErrorKind::Closed,Outcome::NotSent)));break ClientErrorKind::Closed;}
                if tokio::time::Instant::now()>=call.deadline {let _=call.reply.send(Err(error(ClientErrorKind::Timeout,Outcome::NotSent)));continue;}
                if startup.claim.live().is_err() {let _=call.reply.send(Err(error(ClientErrorKind::Session,Outcome::NotSent)));break ClientErrorKind::Session;}
                let Some(id)=next_id.checked_add(1) else {let _=call.reply.send(Err(error(ClientErrorKind::Capacity,Outcome::NotSent)));break ClientErrorKind::Capacity;};
                next_id=id;
                let reply=Arc::new(Mutex::new(Some(call.reply)));
                let slot=Arc::new(Mutex::new(Some(call.permit)));
                let state=Arc::new(CallState::default());
                active.insert(id,Pending {reply:reply.clone(),_slot:slot.clone(),deadline:call.deadline,state:state.clone()});
                let context=Context {handle:handle.clone(),session:startup.claim.session(),max_response_bytes:options.max_response_bytes,state:state.clone(),shared:shared.clone(),deadline:call.deadline,blocking:blocking.clone(),#[cfg(test)]before_compute:options.before_compute,#[cfg(test)]before_dc_confirm:options.before_dc_confirm};
                let shared=shared.clone();
                let session_health=context.handle.session.clone();
                #[cfg(test)]
                let after_reply=options.after_reply;
                #[cfg(test)]
                let before_fence=options.before_fence;
                #[cfg(test)]
                let before_accept=options.before_accept;
                jobs.spawn(async move {
                    let result=if tokio::time::Instant::now()>=call.deadline {Err(error(ClientErrorKind::Timeout,state.seal()))}
                        else if shared.closed.load(Ordering::Acquire) {Err(error(ClientErrorKind::Closed,state.seal()))}
                        else {(call.action)(context).await};
                    #[cfg(test)]
                    if let Some(delay)=before_fence {tokio::time::sleep(delay).await;}
                    #[cfg(test)]
                    if let Some(delay)=before_accept {std::thread::sleep(delay);}
                    // Completion and shutdown sealing have one decision point.
                    // A supervisor failure cannot be overwritten by a late job.
                    let (override_failure,fence)={
                        let mut completion=state.completion.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *completion!=Completion::Pending {drop(completion);return (id,None);}
                        let healthy=session_health.home_dc_id().is_ok();
                        let override_failure=if tokio::time::Instant::now()>=call.deadline {Some(error(ClientErrorKind::Timeout,state.seal()))}
                            else if !healthy {Some(error(ClientErrorKind::Session,state.seal()))}
                            else if result.is_ok() && shared.closed.load(Ordering::Acquire) {Some(error(ClientErrorKind::Closed,state.seal()))}
                            else {None};
                        let failure=override_failure.or_else(||result.as_ref().err().copied());
                        *completion=Completion::Accepted;
                        state.done.store(true,Ordering::Release);
                        if override_failure.is_none() && result.is_ok() && state.logout_confirmed.load(Ordering::Acquire) {shared.logout_confirmed.store(true,Ordering::Release);}
                        let fence=if state.stop_generation.load(Ordering::Acquire) {Some(ClientErrorKind::Closed)} else {failure.as_ref().filter(|failure|failure.kind()==ClientErrorKind::Timeout || (failure.outcome()==Outcome::Unknown && (state.authorization_transition() || matches!(failure.kind(),ClientErrorKind::Backend|ClientErrorKind::Session|ClientErrorKind::Decode|ClientErrorKind::Capacity|ClientErrorKind::IdentityMismatch)))).map(ClientError::kind)};
                        if fence.is_some() {shared.closed.store(true,Ordering::Release);}
                        (override_failure,fence)
                    };
                    state.release_authorization();
                    if fence.is_none() {
                        // The backend has completed. Free admission before the caller
                        // observes its receipt so sequential calls cannot spuriously
                        // fail capacity. Fenced work stays owned through driver join.
                        let permit=slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                        drop(permit);
                    }
                    // Returned user types can panic on Drop after cancellation. Keep
                    // delivery/destruction in the owned job, never in the supervisor.
                    if let Some(failure)=override_failure {
                        if let Some(sender)=reply_take(&reply) {let _=sender.send(Err(failure));}
                        // No user-defined destructor runs under the completion lock.
                        drop(result);
                    } else if let Some(sender)=reply_take(&reply) {let _=sender.send(result);}
                    #[cfg(test)]
                    if let Some(delay)=after_reply {tokio::time::sleep(delay).await;}
                    (id,fence)
                });
            }
        }
    };
    shared.closed.store(true, Ordering::Release);
    slots.close();
    startup.receiver.close();
    for pending in active.values_mut() {
        if let Some(outcome) = pending.state.seal_completion()
            && let Some(reply) = reply_take(&pending.reply)
        {
            let _ = reply.send(Err(error(reason, outcome)));
        }
    }
    while let Ok(call) = startup.receiver.try_recv() {
        let _ = call.reply.send(Err(error(reason, Outcome::NotSent)));
    }
    handle.quit();
    let driver_result = match driver_exit {
        Some(result) => result,
        None => driver.await,
    };
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    let mut blocking_failed = false;
    {
        let mut jobs = blocking.lock().await;
        jobs.abort_all();
        while let Some(result) = jobs.join_next().await {
            blocking_failed |= result.is_err_and(|error| error.is_panic());
        }
    }
    drop(active);
    startup.updates.close().await;
    drop(handle);
    let stored = if shared.logout_confirmed.load(Ordering::Acquire) {
        startup.claim.finish_logged_out().await
    } else {
        startup.claim.finish().await
    }
    .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown));
    let result = driver_result
        .map_err(|_| error(ClientErrorKind::Backend, Outcome::Unknown))
        .and(stored)
        .and(if blocking_failed {
            Err(error(ClientErrorKind::Backend, Outcome::Unknown))
        } else {
            Ok(())
        });
    shared.exited.send_replace(Some(result));
    exit.complete();
}
