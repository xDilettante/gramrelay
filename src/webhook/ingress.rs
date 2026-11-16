use super::{WebhookError, WebhookOptions, WebhookSecret, WebhookSink};
use crate::Result;
use futures_util::FutureExt;
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tokio::{sync::Notify, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Admission {
    closed: bool,
    active: usize,
    bytes: usize,
}
struct Inner {
    secret: WebhookSecret,
    options: WebhookOptions,
    sink: Arc<dyn WebhookSink>,
    admission: Mutex<Admission>,
    changed: Notify,
    abort: CancellationToken,
}
/// Framework-neutral authentication/admission boundary. Managed source ownership is separate.
#[derive(Clone)]
pub struct WebhookIngress {
    inner: Arc<Inner>,
}
impl fmt::Debug for WebhookIngress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookIngress").finish_non_exhaustive()
    }
}
impl WebhookIngress {
    pub fn new(
        secret: WebhookSecret,
        options: WebhookOptions,
        sink: impl WebhookSink,
    ) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            inner: Arc::new(Inner {
                secret,
                options,
                sink: Arc::new(sink),
                admission: Mutex::new(Admission::default()),
                changed: Notify::new(),
                abort: CancellationToken::new(),
            }),
        })
    }
    /// Authenticate and reserve one full event budget before a transport reads its body.
    pub fn begin(
        &self,
        secret: Option<&[u8]>,
    ) -> std::result::Result<WebhookRequest, WebhookError> {
        if !self.inner.secret.matches(secret) {
            return Err(WebhookError::Unauthorized);
        }
        let deadline = Instant::now()
            .checked_add(self.inner.options.request_timeout)
            .ok_or(WebhookError::Unavailable)?;
        let mut admission = self
            .inner
            .admission
            .lock()
            .map_err(|_| WebhookError::Unavailable)?;
        if admission.closed {
            return Err(WebhookError::Closed);
        }
        let bytes = admission
            .bytes
            .checked_add(self.inner.options.max_body_bytes)
            .ok_or(WebhookError::Busy)?;
        if admission.active >= self.inner.options.max_in_flight
            || bytes > self.inner.options.max_buffered_bytes
        {
            return Err(WebhookError::Busy);
        }
        admission.active += 1;
        admission.bytes = bytes;
        Ok(WebhookRequest {
            inner: self.inner.clone(),
            deadline,
        })
    }
    pub fn close_admission(&self) {
        if let Ok(mut state) = self.inner.admission.lock() {
            state.closed = true;
        }
        self.inner.changed.notify_waiters();
    }
    /// Stop admission and cancel request futures; backend-owned jobs remain its responsibility.
    pub fn abort(&self) {
        self.close_admission();
        self.inner.abort.cancel();
    }
    /// Drain admitted request lifetimes. Managed modes also drain/fence their backend jobs.
    pub async fn shutdown(&self) -> std::result::Result<(), WebhookError> {
        self.close_admission();
        let drain = async {
            loop {
                let notified = self.inner.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self
                    .inner
                    .admission
                    .lock()
                    .map_err(|_| WebhookError::Unavailable)?
                    .active
                    == 0
                {
                    return Ok(());
                }
                notified.await;
            }
        };
        match tokio::time::timeout(self.inner.options.shutdown_timeout, drain).await {
            Ok(result) => result,
            Err(_) => {
                self.abort();
                Err(WebhookError::Timeout)
            }
        }
    }
    pub(super) fn options(&self) -> &WebhookOptions {
        &self.inner.options
    }
}
/// One admitted request; dropping it releases capacity on every exit/cancellation path.
pub struct WebhookRequest {
    inner: Arc<Inner>,
    pub(super) deadline: Instant,
}
impl fmt::Debug for WebhookRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookRequest").finish_non_exhaustive()
    }
}
impl WebhookRequest {
    pub fn max_body_bytes(&self) -> usize {
        self.inner.options.max_body_bytes
    }
    pub fn remaining(&self) -> std::result::Result<std::time::Duration, WebhookError> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(WebhookError::Timeout)
    }
    pub(super) async fn cancelled(&self) {
        self.inner.abort.cancelled().await;
    }
    pub async fn deliver(self, body: &[u8]) -> std::result::Result<(), WebhookError> {
        if self.inner.abort.is_cancelled() {
            return Err(WebhookError::Unavailable);
        }
        self.remaining()?;
        if body.len() > self.max_body_bytes() {
            return Err(WebhookError::PayloadTooLarge);
        }
        let update: crate::Update =
            serde_json::from_slice(body).map_err(|_| WebhookError::BadRequest)?;
        if update.update_id < 0 || update.known_event_count() > 1 {
            return Err(WebhookError::BadRequest);
        }
        self.remaining()?;
        let sink = self.inner.sink.clone();
        let work =
            std::panic::AssertUnwindSafe(async move { sink.ingest(update).await }).catch_unwind();
        tokio::select! {
            _=self.cancelled()=>Err(WebhookError::Unavailable),
            result=tokio::time::timeout_at(self.deadline,work)=>result.map_err(|_|WebhookError::Timeout)?.map_err(|_|WebhookError::Handler)?,
        }
    }
}
impl Drop for WebhookRequest {
    fn drop(&mut self) {
        if let Ok(mut admission) = self.inner.admission.lock() {
            admission.active -= 1;
            admission.bytes -= self.inner.options.max_body_bytes;
        }
        self.inner.changed.notify_waiters();
    }
}
