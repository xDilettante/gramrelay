mod queue;
#[cfg(test)]
mod tests;
mod types;
mod worker;
use super::{WebhookIngress, WebhookOptions, WebhookSecret};
use crate::{Bot, CancellationToken, EffectOutcome, Error, ErrorKind, Result, Router};
use queue::MemorySink;
use std::{fmt, sync::Arc};
use tokio::{net::TcpListener, task::JoinHandle, time::Instant};
pub use types::MemoryWebhookOptions;

/// Lazy managed webhook for an ordinary bot. Startup verifies getMe and claims the SDK source.
pub struct MemoryWebhook<S = ()> {
    bot: Bot,
    router: Router<S>,
    secret: WebhookSecret,
    options: WebhookOptions,
    memory: MemoryWebhookOptions,
}
impl<S> fmt::Debug for MemoryWebhook<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryWebhook").finish_non_exhaustive()
    }
}
impl Bot {
    pub fn webhook<S: Send + Sync + 'static>(
        &self,
        router: Router<S>,
        secret: WebhookSecret,
    ) -> MemoryWebhook<S> {
        MemoryWebhook {
            bot: self.clone(),
            router,
            secret,
            options: WebhookOptions::default(),
            memory: MemoryWebhookOptions::default(),
        }
    }
}
impl<S: Send + Sync + 'static> MemoryWebhook<S> {
    pub fn options(mut self, options: WebhookOptions) -> Self {
        self.options = options;
        self
    }
    pub fn memory_options(mut self, options: MemoryWebhookOptions) -> Self {
        self.memory = options;
        self
    }
    /// Stop the token and await this future to join owned work before source release.
    /// Restart loses this memory cache; external effects need stable operation IDs.
    pub async fn serve_until(self, listener: TcpListener, stop: CancellationToken) -> Result<()> {
        let Self {
            bot,
            mut router,
            secret,
            options,
            memory,
        } = self;
        options.validate_listener(&listener)?;
        memory.validate()?;
        router.validate()?;
        if stop.is_cancelled() {
            return Ok(());
        }
        let _source = bot.claim_update_source()?;
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            result = bot.prepare_router(&mut router) => { result?; },
        }
        let (sink, receiver) = MemorySink::new(memory.clone());
        let ingress = WebhookIngress::new(secret, options.clone(), sink.clone())?;
        let abort = CancellationToken::new();
        let task = tokio::spawn(worker::run(
            bot,
            Arc::new(router),
            sink.clone(),
            receiver,
            memory,
            abort.clone(),
        ));
        let mut driver = Driver {
            sink,
            abort,
            task: Some(task),
        };
        let serving_stop = stop.child_token();
        let serving = ingress.serve_until(listener, serving_stop.clone());
        tokio::pin!(serving);
        let (served, deadline, worker_failure) = tokio::select! {
            result = &mut serving => (result, shutdown_deadline(options.shutdown_timeout)?, None),
            result = driver.join() => {
                let deadline = shutdown_deadline(options.shutdown_timeout)?;
                serving_stop.cancel();
                (serving.await, deadline, Some(result.err().unwrap_or_else(worker_error)))
            }
            _ = stop.cancelled() => {
                let deadline = shutdown_deadline(options.shutdown_timeout)?;
                (serving.await, deadline, None)
            }
        };
        let finished = driver
            .finish(deadline, served.is_err() || worker_failure.is_some())
            .await;
        if let Some(error) = worker_failure {
            return Err(error);
        }
        served?;
        finished
    }
}
struct Driver {
    sink: Arc<MemorySink>,
    abort: CancellationToken,
    task: Option<JoinHandle<Result<()>>>,
}
impl Driver {
    async fn join(&mut self) -> Result<()> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task.await.map_err(|_| worker_error());
        self.task.take();
        result?
    }
    async fn finish(&mut self, deadline: Instant, force: bool) -> Result<()> {
        self.sink.close();
        if force {
            self.abort.cancel();
        }
        match tokio::time::timeout_at(deadline, self.join()).await {
            Ok(result) => result,
            Err(_) => {
                self.abort.cancel();
                let _ = self.join().await;
                Err(Error::new(ErrorKind::Timeout, EffectOutcome::Unknown))
            }
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.sink.close();
        self.abort.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
fn worker_error() -> Error {
    Error::new(ErrorKind::Handler, EffectOutcome::Unknown)
}
fn shutdown_deadline(timeout: std::time::Duration) -> Result<Instant> {
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::local(ErrorKind::Validation, "shutdown_timeout"))
}
