use super::*;
use crate::session::Health;
use grammers_mtsender::SenderPoolFatHandle;
use std::collections::HashMap;
use tl::Deserializable;
use tokio::task::JoinSet;

#[derive(Clone)]
pub(super) struct Context {
    handle: SenderPoolFatHandle,
    max_response_bytes: usize,
    state: Arc<CallState>,
    shared: Arc<Shared>,
    deadline: tokio::time::Instant,
}
impl Context {
    pub async fn invoke<R: tl::RemoteCall>(&self, body: Vec<u8>) -> Result<R::Return> {
        let dc = self
            .handle
            .session
            .home_dc_id()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?;
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
        let bytes = self
            .handle
            .raw_invoke_in_dc(dc, body)
            .await
            .map_err(super::error::invocation)?;
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
#[derive(Default)]
struct CallState {
    // Queue admission and sealing share one linearization point. Once backend
    // admission is attempted, only conservative outcomes may be reported.
    phase: std::sync::atomic::AtomicU8,
    done: AtomicBool,
}
impl CallState {
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
    // Updates stay owned until the observable receiver is installed in the next increment.
    let _updates = pool.updates;
    let mut health = startup.claim.health();
    let mut jobs = JoinSet::new();
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
                if active.values().any(|pending|!pending.state.done.load(Ordering::Acquire) && pending.deadline<=now) {break ClientErrorKind::Timeout;}
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
                let context=Context {handle:handle.clone(),max_response_bytes:options.max_response_bytes,state:state.clone(),shared:shared.clone(),deadline:call.deadline};
                let shared=shared.clone();
                let session_health=context.handle.session.clone();
                #[cfg(test)]
                let after_reply=options.after_reply;
                jobs.spawn(async move {
                    let result=if tokio::time::Instant::now()>=call.deadline {Err(error(ClientErrorKind::Timeout,state.seal()))}
                        else if shared.closed.load(Ordering::Acquire) {Err(error(ClientErrorKind::Closed,state.seal()))}
                        else {(call.action)(context).await};
                    let result=if session_health.home_dc_id().is_err() {Err(error(ClientErrorKind::Session,state.seal()))}else{result};
                    let result=if tokio::time::Instant::now()>=call.deadline {Err(error(ClientErrorKind::Timeout,state.seal()))}else{result};
                    state.done.store(true,Ordering::Release);
                    let fence=result.as_ref().err().filter(|failure|failure.kind()==ClientErrorKind::Timeout || (failure.outcome()==Outcome::Unknown && matches!(failure.kind(),ClientErrorKind::Backend|ClientErrorKind::Session|ClientErrorKind::Decode|ClientErrorKind::Capacity))).map(ClientError::kind);
                    if fence.is_some() {shared.closed.store(true,Ordering::Release);}
                    else {
                        // The backend has completed. Free admission before the caller
                        // observes its receipt so sequential calls cannot spuriously
                        // fail capacity. Fenced work stays owned through driver join.
                        let permit=slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                        drop(permit);
                    }
                    // Returned user types can panic on Drop after cancellation. Keep
                    // delivery/destruction in the owned job, never in the supervisor.
                    if let Some(sender)=reply_take(&reply) {let _=sender.send(result);}
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
        if let Some(reply) = reply_take(&pending.reply) {
            let _ = reply.send(Err(error(reason, pending.state.seal())));
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
    drop(active);
    drop(handle);
    let stored = startup
        .claim
        .finish()
        .await
        .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown));
    let result = driver_result
        .map_err(|_| error(ClientErrorKind::Backend, Outcome::Unknown))
        .and(stored);
    shared.exited.send_replace(Some(result));
    exit.complete();
}
