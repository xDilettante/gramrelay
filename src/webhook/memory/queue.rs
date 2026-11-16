use super::MemoryWebhookOptions;
use crate::{
    ChatId, Update,
    webhook::{WebhookError, WebhookFuture, WebhookSink},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
pub(super) type Outcome = std::result::Result<(), WebhookError>;

struct Record {
    hash: [u8; 32],
    until: Option<Instant>,
}
struct State {
    sender: Option<mpsc::Sender<Job>>,
    records: BTreeMap<i64, Record>,
    expiry: VecDeque<(Instant, i64)>,
    pending: usize,
    bytes: usize,
}
pub(super) struct MemorySink {
    state: Mutex<State>,
    options: MemoryWebhookOptions,
}
pub(super) struct Job {
    pub update: Update,
    pub meta: JobMeta,
}
pub(super) struct JobMeta {
    pub id: i64,
    pub key: Option<ChatId>,
    pub deadline: Instant,
    bytes: usize,
    reply: oneshot::Sender<Outcome>,
}
impl MemorySink {
    #[cfg(test)]
    pub(super) fn pending_count(&self) -> usize {
        self.state.lock().unwrap().pending
    }
    pub fn new(options: MemoryWebhookOptions) -> (Arc<Self>, mpsc::Receiver<Job>) {
        let (sender, receiver) = mpsc::channel(options.max_pending);
        (
            Arc::new(Self {
                state: Mutex::new(State {
                    sender: Some(sender),
                    records: BTreeMap::new(),
                    expiry: VecDeque::new(),
                    pending: 0,
                    bytes: 0,
                }),
                options,
            }),
            receiver,
        )
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.sender.take();
        }
    }
    fn submit(
        &self,
        update: Update,
        deadline: Instant,
    ) -> std::result::Result<Option<oneshot::Receiver<Outcome>>, WebhookError> {
        if Instant::now() >= deadline {
            return Err(WebhookError::Timeout);
        }
        let mut canonical = serde_json::to_value(&update).map_err(|_| WebhookError::BadRequest)?;
        canonical.sort_all_objects();
        let canonical = serde_json::to_vec(&canonical).map_err(|_| WebhookError::BadRequest)?;
        let hash: [u8; 32] = Sha256::digest(&canonical).into();
        let bytes = canonical.len();
        let mut state = self.state.lock().map_err(|_| WebhookError::Unavailable)?;
        if state.sender.is_none() {
            return Err(WebhookError::Closed);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(WebhookError::Timeout);
        }
        while state.expiry.front().is_some_and(|(until, _)| *until <= now) {
            let (_, id) = state.expiry.pop_front().ok_or(WebhookError::Unavailable)?;
            state.records.remove(&id);
        }
        if let Some(record) = state.records.get(&update.update_id) {
            if record.hash != hash {
                return Err(WebhookError::Conflict);
            }
            return if record.until.is_some() {
                Ok(None)
            } else {
                Err(WebhookError::Busy)
            };
        }
        let total = state.bytes.checked_add(bytes).ok_or(WebhookError::Busy)?;
        if state.pending >= self.options.max_pending
            || total > self.options.max_pending_bytes
            || state.records.len() >= self.options.dedup_capacity
        {
            return Err(WebhookError::Busy);
        }
        let (reply, waiting) = oneshot::channel();
        let id = update.update_id;
        let meta = JobMeta {
            id,
            key: update.chat_id(),
            deadline,
            bytes,
            reply,
        };
        state
            .sender
            .as_ref()
            .ok_or(WebhookError::Closed)?
            .try_send(Job { update, meta })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => WebhookError::Busy,
                mpsc::error::TrySendError::Closed(_) => WebhookError::Closed,
            })?;
        state.records.insert(id, Record { hash, until: None });
        state.pending += 1;
        state.bytes = total;
        Ok(Some(waiting))
    }
    pub fn complete(&self, meta: JobMeta, result: Outcome) {
        let outcome = if let Ok(mut state) = self.state.lock() {
            state.pending -= 1;
            state.bytes -= meta.bytes;
            if result.is_ok() {
                let expiry = Instant::now().checked_add(self.options.dedup_retention);
                if let Some(record) = state.records.get_mut(&meta.id) {
                    record.until = expiry;
                }
                if let Some(expiry) = expiry {
                    state.expiry.push_back((expiry, meta.id));
                }
            } else {
                state.records.remove(&meta.id);
            }
            result
        } else {
            Err(WebhookError::Unavailable)
        };
        let _ = meta.reply.send(outcome);
    }
}
impl WebhookSink for Arc<MemorySink> {
    fn ingest(&self, update: Update, deadline: Instant) -> WebhookFuture<'_> {
        Box::pin(async move {
            match self.submit(update, deadline)? {
                Some(waiting) => waiting.await.map_err(|_| WebhookError::Unavailable)?,
                None => Ok(()),
            }
        })
    }
}
