use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// Host clock trust, never a tool argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackClockPolicy {
    /// Requires monotonic ingestion provenance in this journal lifetime.
    JournalMonotonic,
    /// The host guarantees its UTC clock across historical/cross-process reads.
    TrustedWallClock,
}
/// Every clone of an issued reference observes the same irreversible wall time.
#[derive(Clone, Default)]
pub(crate) struct CallbackClock(Arc<AtomicI64>);
impl CallbackClock {
    pub(crate) fn observe(&self, time: i64) -> i64 {
        self.0.fetch_max(time, Ordering::SeqCst).max(time)
    }
    pub(crate) fn highest(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
