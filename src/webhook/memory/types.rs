use crate::{DispatchOptions, Error, ErrorKind, Result};
use std::time::Duration;
use tokio::time::Instant;

/// Logical payload/entry bounds include waiting and active work, even after HTTP disconnects.
#[derive(Debug, Clone)]
pub struct MemoryWebhookOptions {
    pub(super) max_pending: usize,
    pub(super) max_pending_bytes: usize,
    pub(super) dedup_capacity: usize,
    pub(super) dedup_retention: Duration,
    pub(super) dispatch: DispatchOptions,
}
impl Default for MemoryWebhookOptions {
    fn default() -> Self {
        Self {
            max_pending: 100,
            max_pending_bytes: 16 * 1024 * 1024,
            dedup_capacity: 100_000,
            dedup_retention: Duration::from_secs(24 * 3600),
            dispatch: DispatchOptions::default(),
        }
    }
}
impl MemoryWebhookOptions {
    pub fn max_pending(mut self, value: usize) -> Self {
        self.max_pending = value;
        self
    }
    pub fn max_pending_bytes(mut self, value: usize) -> Self {
        self.max_pending_bytes = value;
        self
    }
    pub fn dedup_capacity(mut self, value: usize) -> Self {
        self.dedup_capacity = value;
        self
    }
    pub fn dedup_retention(mut self, value: Duration) -> Self {
        self.dedup_retention = value;
        self
    }
    pub fn dispatch(mut self, value: DispatchOptions) -> Self {
        self.dispatch = value;
        self
    }
    pub fn validate(&self) -> Result<()> {
        self.dispatch.validate()?;
        if !(1..=1000).contains(&self.max_pending)
            || !(1..=1024 * 1024 * 1024).contains(&self.max_pending_bytes)
            || !(1..=1_000_000).contains(&self.dedup_capacity)
            || self.dedup_retention.is_zero()
            || Instant::now().checked_add(self.dedup_retention).is_none()
        {
            return Err(Error::local(
                ErrorKind::Validation,
                "memory_webhook_options",
            ));
        }
        Ok(())
    }
}
