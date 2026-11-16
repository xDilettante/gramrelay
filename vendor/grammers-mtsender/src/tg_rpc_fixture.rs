// Opt-in downstream test dispatch. This module performs no network I/O.
use super::*;
use std::{future::Future, pin::Pin};

type Response = Pin<Box<dyn Future<Output = Result<Vec<u8>, InvocationError>> + Send>>;
pub(super) struct TestRpc {
    pub invoke: Arc<dyn Fn(i32, Vec<u8>) -> Response + Send + Sync>,
    _updates: Arc<TestUpdates>,
}
pub(super) struct TestUpdates {
    sender: mpsc::Sender<UpdatesLike>,
    loss: watch::Sender<bool>,
}

impl SenderPool {
    /// Replaces only RPC dispatch with synthetic owned tasks. No socket is opened.
    ///
    /// Available only with the downstream tg-test-kit feature. The callback must
    /// bound its script/storage; the caller still bounds admission, as in a real
    /// pool. Request bytes and response decoding are the ordinary SDK path.
    /// NoRetries is fixed for this fixture. Quit cancels and joins callbacks.
    /// Construct futures without blocking in the callback factory.
    /// Requests include their four-byte constructor; generated bare function
    /// deserializers expect the remaining payload. Responses use boxed enums,
    /// including their constructor, exactly like Telegram RPC results.
    pub fn with_test_rpc<S, F, Fut>(session: Arc<S>, api_id: i32, callback: F) -> Self
    where
        S: Session + Sized,
        S::Error: std::error::Error + Send + Sync + 'static,
        F: Fn(i32, Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<u8>, InvocationError>> + Send + 'static,
    {
        let mut pool = Self::with_configuration(
            session,
            api_id,
            ConnectionParams {
                retry_policy: Box::new(crate::NoRetries),
                ..ConnectionParams::default()
            },
        );
        let updates = Arc::new(TestUpdates {
            sender: pool.runner.updates_tx.clone(),
            loss: pool.runner.update_loss.clone(),
        });
        pool.handle.thin.3 = Arc::downgrade(&updates);
        pool.runner.test_rpc = Some(TestRpc {
            invoke: Arc::new(move |dc, bytes| Box::pin(callback(dc, bytes))),
            _updates: updates,
        });
        pool
    }
}

impl SenderPoolHandle {
    /// Injects one synthetic update through the actual nonblocking loss path.
    /// Returns false for real pools, stopped pools and a full/closed queue.
    pub fn test_push_update(&self, update: UpdatesLike) -> bool {
        if *self.1.borrow() {
            return false;
        }
        self.3
            .upgrade()
            .is_some_and(|fixture| forward_update(&fixture.sender, &fixture.loss, update).is_ok())
    }
}
