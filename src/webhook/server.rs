use super::{WebhookIngress, WebhookOptions};
use crate::{CancellationToken, EffectOutcome, Error, ErrorKind, Result};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use tokio::{net::TcpListener, task::JoinSet};

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
        self.options().validate_listener(&listener)?;
        let _lifetime = StopOnDrop(self.clone());
        let app = self.router();
        let connection_stop = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut failure = None;
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
                accepted=listener.accept(),if connections.len()<self.options().max_connections=>{
                    let (stream,_peer)=match accepted {
                        Ok(connection)=>connection,
                        Err(_)=>{failure=Some(Error::new(ErrorKind::Io,EffectOutcome::Unknown));break;}
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
