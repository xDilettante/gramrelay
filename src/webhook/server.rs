use super::{WebhookIngress, WebhookOptions};
use crate::{CancellationToken, EffectOutcome, Error, ErrorKind, Result};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{io, net::SocketAddr, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::Instant,
};

const ACCEPT_RESOURCE_DELAY: Duration = Duration::from_millis(100);

struct StopOnDrop(WebhookIngress);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl WebhookIngress {
    /// Serve bounded HTTP/1 behind the host's TLS proxy. Low-level callers own source identity,
    /// ownership and backend workers. Stop admission and drain accepted requests on cancellation.
    pub async fn serve_until(&self, listener: TcpListener, stop: CancellationToken) -> Result<()> {
        self.serve_with_accept(listener, stop, async |listener: &TcpListener| {
            listener.accept().await
        })
        .await
    }

    async fn serve_with_accept(
        &self,
        listener: TcpListener,
        stop: CancellationToken,
        mut accept: impl AsyncFnMut(&TcpListener) -> io::Result<(TcpStream, SocketAddr)>,
    ) -> Result<()> {
        self.options().validate_listener(&listener)?;
        let _lifetime = StopOnDrop(self.clone());
        let app = self.router();
        let connection_stop = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut failure = None;
        let mut accept_after = None;
        loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>break,
                finished=connections.join_next(),if !connections.is_empty()=>{
                    if finished.is_some_and(|joined|joined.is_err()) {
                        failure=Some(Error::new(ErrorKind::Handler,EffectOutcome::Unknown));
                        break;
                    }
                }
                accepted=async {
                    if let Some(when) = accept_after {
                        tokio::time::sleep_until(when).await;
                    }
                    accept(&listener).await
                },if connections.len()<self.options().max_connections=>{
                    let (stream,_peer)=match accepted {
                        Ok(connection)=>{
                            accept_after=None;
                            connection
                        }
                        Err(error)=>{
                            if let Some(delay)=accept_retry_delay(&error) {
                                accept_after=Some(Instant::now()+delay);
                                continue;
                            }
                            failure=Some(Error::new(ErrorKind::Io,EffectOutcome::Unknown));
                            break;
                        }
                    };
                    let _=stream.set_nodelay(true);
                    let app=app.clone();let stop=connection_stop.clone();
                    let header_timeout=self.options().header_timeout;
                    let connection_timeout=self.options().connection_timeout;
                    connections.spawn(async move {
                        let mut builder=http1::Builder::new();
                        builder.timer(TokioTimer::new()).header_read_timeout(header_timeout).max_headers(32).max_buf_size(16*1024);
                        let connection=builder.serve_connection(TokioIo::new(stream),TowerToHyperService::new(app));
                        tokio::pin!(connection);
                        let serve=async {
                            tokio::select! {
                                _=&mut connection=>{},
                                _=stop.cancelled()=>{connection.as_mut().graceful_shutdown();let _=connection.await;}
                            }
                        };
                        // Connection errors belong to this peer; never format raw URLs/headers.
                        let _=tokio::time::timeout(connection_timeout,serve).await;
                    });
                }
            }
        }
        drop(listener);
        self.close_admission();
        connection_stop.cancel();
        let drain = async {
            while let Some(joined) = connections.join_next().await {
                if joined.is_err() && failure.is_none() {
                    failure = Some(Error::new(ErrorKind::Handler, EffectOutcome::Unknown));
                }
            }
            self.shutdown()
                .await
                .map_err(|_| Error::new(ErrorKind::Timeout, EffectOutcome::Unknown))
        };
        let drained = tokio::time::timeout(self.options().shutdown_timeout, drain).await;
        if drained.is_err() {
            self.abort();
            connections.abort_all();
            while connections.join_next().await.is_some() {}
            return Err(Error::new(ErrorKind::Timeout, EffectOutcome::Unknown));
        }
        drained.map_err(|_| Error::new(ErrorKind::Timeout, EffectOutcome::Unknown))??;
        failure.map_or(Ok(()), Err)
    }
}

fn accept_retry_delay(error: &io::Error) -> Option<Duration> {
    match error.kind() {
        io::ErrorKind::Interrupted
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset => Some(Duration::ZERO),
        io::ErrorKind::OutOfMemory => Some(ACCEPT_RESOURCE_DELAY),
        _ if temporary_accept_resource_error(error.raw_os_error()) => Some(ACCEPT_RESOURCE_DELAY),
        _ => None,
    }
}

fn temporary_accept_resource_error(code: Option<i32>) -> bool {
    #[cfg(unix)]
    {
        matches!(code, Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Networking::WinSock::{WSAEMFILE, WSAENOBUFS};
        matches!(code, Some(WSAEMFILE | WSAENOBUFS))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = code;
        false
    }
}

impl WebhookOptions {
    /// Check managed plaintext listener policy before starting identity requests or source claims.
    pub fn validate_listener(&self, listener: &TcpListener) -> Result<()> {
        self.validate()?;
        let address = listener
            .local_addr()
            .map_err(|_| Error::local(ErrorKind::Io, "webhook_listener"))?;
        if !address.ip().is_loopback() && !self.trusted_proxy {
            return Err(Error::local(ErrorKind::Configuration, "webhook_listener"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Update,
        webhook::{WebhookFuture, WebhookSecret, WebhookSink},
    };
    use tokio::sync::Notify;

    const SECRET: &str = "SYNTHETIC_SECRET";

    struct Immediate;

    impl WebhookSink for Immediate {
        fn ingest(&self, _: Update, _: Instant) -> WebhookFuture<'_> {
            Box::pin(async { Ok(()) })
        }
    }

    fn ingress() -> WebhookIngress {
        WebhookIngress::new(
            WebhookSecret::new(SECRET).unwrap(),
            WebhookOptions::default(),
            Immediate,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn server_accepts_a_request_after_a_peer_abort_and_resource_backoff() {
        let ingress = ingress();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let mut attempts = 0;
        let mut resume_at = None;
        let server =
            ingress.serve_with_accept(listener, stop.clone(), async |listener: &TcpListener| {
                attempts += 1;
                match attempts {
                    1 => Err(io::ErrorKind::ConnectionAborted.into()),
                    2 => {
                        resume_at = Some(Instant::now() + ACCEPT_RESOURCE_DELAY);
                        Err(io::ErrorKind::OutOfMemory.into())
                    }
                    _ => {
                        assert!(Instant::now() >= resume_at.unwrap());
                        listener.accept().await
                    }
                }
            });
        let request = async {
            let response = reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .post(format!("http://{address}/telegram"))
                .header("x-telegram-bot-api-secret-token", SECRET)
                .json(&serde_json::json!({"update_id":1}))
                .send()
                .await;
            stop.cancel();
            response
        };
        let (served, response) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(server, request)
        })
        .await
        .unwrap();
        served.unwrap();
        assert_eq!(response.unwrap().status(), 200);
    }

    #[tokio::test]
    async fn cancelling_accept_backoff_does_not_make_another_attempt() {
        let ingress = ingress();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stop = CancellationToken::new();
        let attempted = Notify::new();
        let mut attempts = 0;
        let server = ingress.serve_with_accept(listener, stop.clone(), async |_: &TcpListener| {
            attempts += 1;
            attempted.notify_one();
            Err(io::ErrorKind::OutOfMemory.into())
        });
        let cancel = async {
            attempted.notified().await;
            stop.cancel();
        };
        let (served, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(server, cancel)
        })
        .await
        .unwrap();
        served.unwrap();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn a_fatal_accept_error_ends_the_server() {
        let ingress = ingress();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = ingress
            .serve_with_accept(
                listener,
                CancellationToken::new(),
                async |_: &TcpListener| Err(io::ErrorKind::InvalidInput.into()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Io);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn descriptor_exhaustion_uses_the_resource_backoff() {
        #[cfg(unix)]
        let code = libc::EMFILE;
        #[cfg(windows)]
        let code = windows_sys::Win32::Networking::WinSock::WSAEMFILE;
        assert_eq!(
            accept_retry_delay(&io::Error::from_raw_os_error(code)),
            Some(ACCEPT_RESOURCE_DELAY)
        );
    }
}
