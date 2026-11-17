pub mod auth;
mod common;
mod error;
mod runtime;
#[cfg(feature = "test-kit")]
pub mod test_kit;
#[cfg(test)]
mod tests;
mod updates;
use crate::{Session, session::ClientClaim};
pub use auth::{AppCredentials, BotToken};
pub use common::{
    Account, CallbackAnswer, Dialog, DialogContinuation, DialogCursor, DialogFolder, DialogId,
    DialogPage, DialogPageIssue, DocumentReceipt, DownloadOptions, DownloadReceipt,
    DownloadedDocument, HistoryCursor, HistoryMessage, HistoryMessageKind, HistoryPage,
    MessageReceipt, Peer, PeerId, PeerKind, RemoteDocument, SendDocument, SendText,
};
pub use error::{ClientError, ClientErrorKind, Outcome};
use error::{Result, error};
use grammers_mtsender::{ConnectionParams, NoRetries, SenderPool};
use grammers_tl_types as tl;
use std::{
    any::Any,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
pub use updates::{
    ResyncRequired, UpdateBatch, UpdateBatchKind, UpdateGapReason, UpdateItem, UpdateMode,
    UpdateReceiver, UpdateScope,
};
use zeroize::Zeroizing;

#[derive(Clone, Debug)]
pub struct ClientOptions {
    #[cfg(test)]
    after_reply: Option<Duration>,
    #[cfg(test)]
    before_fence: Option<Duration>,
    #[cfg(test)]
    before_accept: Option<Duration>,
    #[cfg(test)]
    before_compute: Option<Duration>,
    #[cfg(test)]
    before_dc_confirm: Option<Duration>,
    max_pending: usize,
    timeout: Duration,
    max_request_bytes: usize,
    max_response_bytes: usize,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            after_reply: None,
            #[cfg(test)]
            before_fence: None,
            #[cfg(test)]
            before_accept: None,
            #[cfg(test)]
            before_compute: None,
            #[cfg(test)]
            before_dc_confirm: None,
            max_pending: 16,
            timeout: Duration::from_secs(30),
            max_request_bytes: grammers_mtproto::MAX_REQUEST_SIZE.min(1024 * 1024),
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}
impl ClientOptions {
    pub fn max_pending(mut self, value: usize) -> Self {
        self.max_pending = value;
        self
    }
    pub fn request_timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }
    pub fn max_request_bytes(mut self, value: usize) -> Self {
        self.max_request_bytes = value;
        self
    }
    pub fn max_response_bytes(mut self, value: usize) -> Self {
        self.max_response_bytes = value;
        self
    }
    fn validate(&self) -> Result<()> {
        if self.max_pending == 0
            || self.max_pending > 1024
            || self.timeout.is_zero()
            || self.timeout > Duration::from_secs(300)
            || self.max_request_bytes < 4
            || self.max_request_bytes > grammers_mtproto::MAX_REQUEST_SIZE
            || self.max_response_bytes < 4
            || self.max_response_bytes > 64 * 1024 * 1024
        {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(())
    }
}
type Value = Box<dyn Any + Send>;
type JobFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
type Action = Box<dyn FnOnce(runtime::Context) -> JobFuture + Send>;
type Factory = Box<dyn FnOnce(Arc<crate::session::BackendSession>, i32) -> SenderPool + Send>;
struct Call {
    action: Action,
    reply: oneshot::Sender<Result<Value>>,
    permit: OwnedSemaphorePermit,
    deadline: tokio::time::Instant,
}
struct Startup {
    updates: Arc<updates::Hub>,
    claim: ClientClaim,
    receiver: mpsc::Receiver<Call>,
    factory: Factory,
}
struct Shared {
    closed: AtomicBool,
    stop: watch::Sender<bool>,
    exited: watch::Sender<Option<Result<()>>>,
    logout_confirmed: AtomicBool,
}
struct Core {
    updates: Arc<updates::Hub>,
    session: Session,
    authorization: Arc<tokio::sync::RwLock<()>>,
    dc_authorization: Arc<tokio::sync::Mutex<()>>,
    auth_epoch: Arc<AtomicU64>,
    options: ClientOptions,
    startup: Mutex<Option<Startup>>,
    sender: mpsc::Sender<Call>,
    slots: Arc<Semaphore>,
    shared: Arc<Shared>,
}
impl Core {
    fn ensure_actor(&self) -> Result<()> {
        let mut startup = self
            .startup
            .lock()
            .map_err(|_| error(ClientErrorKind::Backend, Outcome::NotSent))?;
        if startup.is_none() {
            return Ok(());
        }
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| error(ClientErrorKind::Runtime, Outcome::NotSent))?;
        if !self.shared.closed.load(Ordering::Acquire) {
            let timer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tokio::time::sleep(Duration::ZERO)
            }))
            .map_err(|_| error(ClientErrorKind::Runtime, Outcome::NotSent))?;
            drop(timer);
        }
        let mut start = startup
            .take()
            .ok_or_else(|| error(ClientErrorKind::Backend, Outcome::NotSent))?;
        start.claim.start();
        let exit = runtime::ExitGuard::new(self.shared.clone(), self.slots.clone());
        handle.spawn(runtime::run(
            start,
            self.options.clone(),
            self.slots.clone(),
            self.shared.clone(),
            exit,
        ));
        Ok(())
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.slots.close();
        self.shared.stop.send_replace(true);
    }
}

/// One owned MTProto generation. Raw TL access is a trusted Rust capability.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Core>,
}
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Client([REDACTED])")
    }
}
impl Client {
    /// Configured RPC-job budget, including queue and identity waits.
    pub fn request_budget(&self) -> Duration {
        self.inner.options.timeout
    }
    /// Claims the session without starting a runtime task or connecting.
    pub fn new(session: Session, options: ClientOptions) -> Result<Self> {
        let response_limit = options.max_response_bytes;
        Self::with_factory(
            session,
            options,
            Box::new(move |session, api_id| {
                SenderPool::with_configuration(
                    session,
                    api_id,
                    ConnectionParams {
                        retry_policy: Box::new(NoRetries),
                        max_response_bytes: response_limit,
                        max_expanded_bytes: response_limit.saturating_mul(2).max(16 * 1024 * 1024),
                        ..ConnectionParams::default()
                    },
                )
            }),
        )
    }
    fn with_factory(session: Session, options: ClientOptions, factory: Factory) -> Result<Self> {
        options.validate()?;
        let claim = session
            .claim_client()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?;
        if session
            .is_quarantined()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?
        {
            return Err(error(ClientErrorKind::Quarantined, Outcome::NotSent));
        }
        let (sender, receiver) = mpsc::channel(options.max_pending);
        let update_hub = Arc::new(updates::Hub::default());
        let shared = Arc::new(Shared {
            closed: AtomicBool::new(false),
            stop: watch::channel(false).0,
            exited: watch::channel(None).0,
            logout_confirmed: AtomicBool::new(false),
        });
        Ok(Self {
            inner: Arc::new(Core {
                updates: update_hub.clone(),
                session,
                authorization: Arc::new(tokio::sync::RwLock::new(())),
                dc_authorization: Arc::new(tokio::sync::Mutex::new(())),
                auth_epoch: Arc::new(AtomicU64::new(0)),
                slots: Arc::new(Semaphore::new(options.max_pending)),
                options,
                startup: Mutex::new(Some(Startup {
                    updates: update_hub,
                    claim,
                    receiver,
                    factory,
                })),
                sender,
                shared,
            }),
        })
    }
    /// Invoke a pinned-layer TL method without automatic retries.
    /// Unknown methods are conservatively treated as writes. No agent tool exposes this API.
    pub async fn invoke<R>(&self, request: &R) -> Result<R::Return>
    where
        R: tl::RemoteCall + Sync,
        R::Return: Send + 'static,
    {
        let reservation = self.reserve()?;
        let mut bytes = encode_request(request, self.inner.options.max_request_bytes)?;
        let authorization = self.inner.authorization.clone();
        self.submit(reservation, move |context| async move {
            context.hold_authorization_read(authorization).await;
            context.invoke::<R>(std::mem::take(&mut *bytes)).await
        })
        .await
    }
    fn reserve(&self) -> Result<(OwnedSemaphorePermit, tokio::time::Instant)> {
        if self.inner.shared.closed.load(Ordering::Acquire) {
            return Err(error(ClientErrorKind::Closed, Outcome::NotSent));
        }
        if self
            .inner
            .session
            .is_quarantined()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?
        {
            return Err(error(ClientErrorKind::Quarantined, Outcome::NotSent));
        }
        let permit = self
            .inner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|failure| {
                error(
                    if failure == tokio::sync::TryAcquireError::Closed {
                        ClientErrorKind::Closed
                    } else {
                        ClientErrorKind::Capacity
                    },
                    Outcome::NotSent,
                )
            })?;
        Ok((
            permit,
            tokio::time::Instant::now() + self.inner.options.timeout,
        ))
    }
    async fn submit<T, F, Fut>(
        &self,
        reservation: (OwnedSemaphorePermit, tokio::time::Instant),
        action: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(runtime::Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        if tokio::time::Instant::now() >= reservation.1 {
            return Err(error(ClientErrorKind::Timeout, Outcome::NotSent));
        }
        self.inner.ensure_actor()?;
        if self.inner.shared.closed.load(Ordering::Acquire) {
            return Err(error(ClientErrorKind::Closed, Outcome::NotSent));
        }
        let (reply, waiting) = oneshot::channel();
        let call = Call {
            action: Box::new(move |context| {
                Box::pin(async move { action(context).await.map(|value| Box::new(value) as Value) })
            }),
            reply,
            permit: reservation.0,
            deadline: reservation.1,
        };
        self.inner
            .sender
            .try_send(call)
            .map_err(|_| error(ClientErrorKind::Closed, Outcome::NotSent))?;
        waiting
            .await
            .map_err(|_| error(ClientErrorKind::Backend, Outcome::Unknown))??
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| error(ClientErrorKind::Backend, Outcome::Unknown))
    }
    /// Stop admission, return conservative outcomes and await all owned cleanup.
    /// Cancellation of this wait does not cancel the cleanup task.
    pub async fn close(&self) -> Result<()> {
        self.inner.shared.closed.store(true, Ordering::Release);
        self.inner.slots.close();
        self.inner.shared.stop.send_replace(true);
        self.inner.ensure_actor()?;
        let mut exited = self.inner.shared.exited.subscribe();
        loop {
            if let Some(result) = *exited.borrow_and_update() {
                return result;
            }
            exited
                .changed()
                .await
                .map_err(|_| error(ClientErrorKind::Backend, Outcome::Unknown))?;
        }
    }
}

fn encode_request<R: tl::Serializable>(request: &R, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut overflow = false;
    request.serialize(&mut |part| {
        if overflow
            || bytes
                .len()
                .checked_add(part.len())
                .is_none_or(|len| len > limit)
        {
            overflow = true;
        } else {
            let required = bytes.len() + part.len();
            if required > bytes.capacity() {
                let capacity = bytes
                    .capacity()
                    .max(32)
                    .saturating_mul(2)
                    .max(required)
                    .min(limit);
                let mut grown = Zeroizing::new(Vec::with_capacity(capacity));
                grown.extend_from_slice(&bytes);
                // Dropping the previous owner wipes its allocation before free.
                bytes = grown;
            }
            bytes.extend_from_slice(part);
        }
    });
    if overflow || bytes.len() < 4 || !bytes.len().is_multiple_of(4) {
        return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
    }
    Ok(bytes)
}
