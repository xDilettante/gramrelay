use super::{
    MemoryWebhookOptions,
    queue::{Job, JobMeta, MemorySink, Outcome},
};
use crate::{
    Bot, CancellationToken, EffectOutcome, Error, ErrorKind, Result, Router, webhook::WebhookError,
};
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    sync::mpsc,
    task::{Id, JoinError, JoinSet},
    time::Instant,
};

pub(super) async fn run<S: Send + Sync + 'static>(
    bot: Bot,
    router: Arc<Router<S>>,
    sink: Arc<MemorySink>,
    mut receiver: mpsc::Receiver<Job>,
    options: MemoryWebhookOptions,
    stop: CancellationToken,
) -> Result<()> {
    let mut pending: VecDeque<Job> = VecDeque::new();
    let mut running = JoinSet::new();
    let mut metadata: Vec<(Id, JobMeta)> = Vec::new();
    let mut ended = false;
    loop {
        while let Ok(job) = receiver.try_recv() {
            pending.push_back(job);
        }
        let mut index = 0;
        while index < pending.len() {
            if pending[index].meta.deadline <= Instant::now() {
                let job = pending.remove(index).ok_or_else(unavailable)?;
                sink.complete(job.meta, Err(WebhookError::Timeout));
            } else {
                index += 1;
            }
        }
        while metadata.len() < options.dispatch.concurrency_limit() && !stop.is_cancelled() {
            let Some(position) = eligible(&pending, &metadata) else {
                break;
            };
            let job = pending.remove(position).ok_or_else(unavailable)?;
            let bot = bot.clone();
            let router = router.clone();
            let dispatch = options.dispatch.clone();
            let cancel = stop.child_token();
            let deadline = job.meta.deadline;
            let handle = running.spawn(async move {
                if Instant::now() >= deadline {
                    return Err(WebhookError::Timeout);
                }
                let work =
                    bot.dispatch_batch_until(router, vec![job.update], &dispatch, cancel.clone());
                tokio::pin!(work);
                tokio::select! {
                    result = &mut work => result.map_err(|error| match error.kind() {
                        ErrorKind::Timeout => WebhookError::Timeout,
                        _ => WebhookError::Handler,
                    }),
                    _ = tokio::time::sleep_until(deadline) => {
                        cancel.cancel();
                        let _ = work.await;
                        Err(WebhookError::Timeout)
                    }
                }
            });
            metadata.push((handle.id(), job.meta));
        }
        if ended && pending.is_empty() && metadata.is_empty() {
            return Ok(());
        }
        let next_expiry = pending.iter().map(|job| job.meta.deadline).min();
        tokio::select! {
            biased;
            _ = stop.cancelled() => {
                sink.close();
                receiver.close();
                while let Ok(job) = receiver.try_recv() {
                    pending.push_back(job);
                }
                for job in pending.drain(..) {
                    sink.complete(job.meta, Err(WebhookError::Unavailable));
                }
                // Child cancellation reaches dispatch_batch_until, which joins handler tasks.
                while let Some(joined) = running.join_next_with_id().await {
                    complete(&sink, &mut metadata, joined)?;
                }
                return Ok(());
            }
            joined = running.join_next_with_id(), if !metadata.is_empty() => {
                if let Some(joined) = joined {
                    complete(&sink, &mut metadata, joined)?;
                }
            }
            _ = async {
                if let Some(deadline) = next_expiry {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
            job = receiver.recv(), if !ended => {
                if let Some(job) = job {
                    pending.push_back(job);
                } else {
                    ended = true;
                }
            }
        }
    }
}
fn eligible(pending: &VecDeque<Job>, running: &[(Id, JobMeta)]) -> Option<usize> {
    if running.iter().any(|(_, job)| job.key.is_none()) {
        return None;
    }
    let barrier = pending
        .iter()
        .position(|job| job.meta.key.is_none())
        .unwrap_or(pending.len());
    if barrier == 0 {
        return (running.is_empty() && !pending.is_empty()).then_some(0);
    }
    pending
        .iter()
        .take(barrier)
        .position(|job| !running.iter().any(|(_, active)| active.key == job.meta.key))
}
fn complete(
    sink: &MemorySink,
    metadata: &mut Vec<(Id, JobMeta)>,
    joined: std::result::Result<(Id, Outcome), JoinError>,
) -> Result<()> {
    let (id, result) = match joined {
        Ok(value) => value,
        Err(error) => (error.id(), Err(WebhookError::Handler)),
    };
    let position = metadata
        .iter()
        .position(|(existing, _)| *existing == id)
        .ok_or_else(unavailable)?;
    let (_, meta) = metadata.swap_remove(position);
    sink.complete(meta, result);
    Ok(())
}
fn unavailable() -> Error {
    Error::new(ErrorKind::Handler, EffectOutcome::Unknown)
}
