use super::{CancellationToken, MAX_FRAME_BYTES, MAX_PENDING_REQUESTS};
use rmcp::{
    RoleServer,
    model::{ClientNotification, JsonRpcMessage, RequestId},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::{
    collections::HashMap,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Semaphore,
};

struct Pending {
    cancelled: bool,
}
pub(super) struct Boundary {
    pending: Mutex<HashMap<RequestId, Pending>>,
    failed: AtomicBool,
    pub stop: CancellationToken,
    pub active: Semaphore,
}
impl Default for Boundary {
    fn default() -> Self {
        Self {
            pending: Default::default(),
            failed: AtomicBool::new(false),
            stop: CancellationToken::new(),
            active: Semaphore::new(MAX_PENDING_REQUESTS),
        }
    }
}
impl Boundary {
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub(super) fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.stop.cancel();
    }
}
pub(super) struct BoundedTransport<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> {
    inner: AsyncRwTransport<RoleServer, super::reader::CheckedReader<R>, TimedWriter<W>>,
    boundary: Arc<Boundary>,
}
impl<R, W> BoundedTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(read: R, write: W, boundary: Arc<Boundary>) -> Self {
        let write = TimedWriter {
            inner: write,
            deadline: None,
            boundary: boundary.clone(),
        };
        let read = super::reader::CheckedReader::new(read, boundary.clone());
        Self {
            inner: AsyncRwTransport::new(read, write).with_max_line_length(MAX_FRAME_BYTES),
            boundary,
        }
    }
}
impl<R, W> Transport<RoleServer> for BoundedTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = io::Error;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), io::Error>> + Send + 'static {
        let id = match &item {
            JsonRpcMessage::Response(response) => Some(response.id.clone()),
            JsonRpcMessage::Error(error) => error.id.clone(),
            _ => None,
        };
        let future = self.inner.send(item);
        let boundary = self.boundary.clone();
        async move {
            let result = tokio::time::timeout(Duration::from_secs(5), future)
                .await
                .unwrap_or_else(|_| Err(io::Error::other("MCP output deadline")));
            if result.is_err() {
                boundary.fail();
            }
            if let Some(id) = id {
                if let Ok(mut pending) = boundary.pending.lock() {
                    // SDK may discard a cancelled response after the handler
                    // finishes. Retire the ID for this connection: reusing it
                    // before SDK consumes that old response can misroute it.
                    if pending.get(&id).is_some_and(|entry| !entry.cancelled) {
                        pending.remove(&id);
                    }
                } else {
                    boundary.fail();
                }
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let message = tokio::select! { biased;
            _ = self.boundary.stop.cancelled() => return None,
            message = self.inner.receive() => message,
        };
        let Some(message) = message else {
            self.boundary.stop.cancel();
            return None;
        };
        let Ok(mut pending) = self.boundary.pending.lock() else {
            self.boundary.fail();
            return None;
        };
        match &message {
            JsonRpcMessage::Request(request) => {
                if pending.len() >= MAX_PENDING_REQUESTS || pending.contains_key(&request.id) {
                    self.boundary.fail();
                    return None;
                }
                pending.insert(request.id.clone(), Pending { cancelled: false });
            }
            JsonRpcMessage::Notification(notification) => {
                if let ClientNotification::CancelledNotification(cancelled) =
                    &notification.notification
                    && let Some(id) = &cancelled.params.request_id
                    && let Some(entry) = pending.get_mut(id)
                {
                    entry.cancelled = true;
                }
            }
            _ => {}
        }
        Some(message)
    }
    async fn close(&mut self) -> Result<(), io::Error> {
        self.boundary.stop.cancel();
        self.inner.close().await
    }
}

// SDK receive() also sends protocol parse errors through this same writer.
// Guard the writer itself so those flushes cannot bypass an output deadline.
struct TimedWriter<W> {
    inner: W,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    boundary: Arc<Boundary>,
}
impl<W> TimedWriter<W> {
    fn ready(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let deadline = self
            .deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_secs(5))));
        if deadline.as_mut().poll(cx).is_ready() {
            self.boundary.fail();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP output deadline",
            ))
        } else {
            Ok(())
        }
    }
    fn checked<T>(&mut self, result: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        match result {
            Poll::Ready(Ok(value)) => {
                self.deadline = None;
                Poll::Ready(Ok(value))
            }
            Poll::Ready(Err(_)) => {
                self.boundary.fail();
                Poll::Ready(Err(io::Error::other("MCP output failed")))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
impl<W: AsyncWrite + Unpin> AsyncWrite for TimedWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.ready(cx)?;
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Ok(0))) && !bytes.is_empty() {
            self.boundary.fail();
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "MCP output failed",
            )));
        }
        self.checked(result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.ready(cx)?;
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        self.checked(result)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.ready(cx)?;
        let result = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.checked(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::AsyncWriteExt;
    struct Stalled;
    impl AsyncWrite for Stalled {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }
    struct Zero;
    impl AsyncWrite for Zero {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(0))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn sdk_owned_parse_error_write_zero_is_not_successful_eof() {
        let (mut input, read) = tokio::io::duplex(1024);
        input.write_all(b"{}\n").await.unwrap();
        let boundary = Arc::new(Boundary::default());
        let mut transport = BoundedTransport::new(read, Zero, boundary.clone());
        assert!(transport.receive().await.is_none());
        assert!(
            boundary.failed(),
            "failed protocol output must not become successful EOF"
        );
    }
    #[tokio::test]
    async fn sdk_owned_parse_error_flush_has_a_deadline() {
        let (mut input, read) = tokio::io::duplex(1024);
        input.write_all(b"{}\n").await.unwrap();
        let boundary = Arc::new(Boundary::default());
        let mut transport = BoundedTransport::new(read, Stalled, boundary.clone());
        let result = tokio::time::timeout(Duration::from_secs(7), transport.receive()).await;
        assert!(
            result.is_ok(),
            "SDK-owned Invalid Request flush must not hang"
        );
        assert!(result.unwrap().is_none());
        assert!(boundary.failed());
    }
    #[tokio::test]
    async fn cancelled_ids_remain_bounded_even_when_the_old_response_flushes() {
        use rmcp::model::ServerResult;
        let (mut input, read) = tokio::io::duplex(8192);
        let boundary = Arc::new(Boundary::default());
        let mut transport = BoundedTransport::new(read, tokio::io::sink(), boundary.clone());
        for id in 1..=MAX_PENDING_REQUESTS {
            input
                .write_all(
                    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").as_bytes(),
                )
                .await
                .unwrap();
            assert!(transport.receive().await.is_some());
            let response = transport.send(JsonRpcMessage::response(
                ServerResult::empty(()),
                RequestId::Number(id as i64),
            ));
            input.write_all(format!("{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{{\"requestId\":{id}}}}}\n").as_bytes()).await.unwrap();
            assert!(transport.receive().await.is_some());
            response.await.unwrap();
        }
        assert_eq!(boundary.pending.lock().unwrap().len(), MAX_PENDING_REQUESTS);
        input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        assert!(transport.receive().await.is_none());
        assert!(boundary.failed());
    }
    #[tokio::test]
    async fn duplicate_active_id_closes_before_a_second_handler_and_flush_releases_normal_ids() {
        use rmcp::model::ServerResult;
        let (mut input, read) = tokio::io::duplex(8192);
        let boundary = Arc::new(Boundary::default());
        let mut transport = BoundedTransport::new(read, tokio::io::sink(), boundary.clone());
        let frame = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        input.write_all(frame).await.unwrap();
        transport.receive().await.unwrap();
        transport
            .send(JsonRpcMessage::response(
                ServerResult::empty(()),
                RequestId::Number(1),
            ))
            .await
            .unwrap();
        assert!(boundary.pending.lock().unwrap().is_empty());
        input.write_all(frame).await.unwrap();
        transport.receive().await.unwrap();
        input.write_all(frame).await.unwrap();
        assert!(transport.receive().await.is_none());
        assert!(boundary.failed());
    }
}
