//! Non-serialized provenance for fresh callback receipts in one journal lifetime.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

type Key = (String, i64);
struct State {
    entries: BTreeMap<Key, Instant>,
    order: VecDeque<(Instant, Key)>,
}
pub(crate) struct ReceiptTracker {
    state: Mutex<State>,
    capacity: usize,
}
impl ReceiptTracker {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                entries: BTreeMap::new(),
                order: VecDeque::new(),
            }),
            capacity,
        }
    }
    pub(crate) fn record(&self, profile: &str, sequences: &[i64], observed: Instant) {
        self.record_at(profile, sequences, observed, Instant::now());
    }
    fn record_at(&self, profile: &str, sequences: &[i64], observed: Instant, now: Instant) {
        if sequences.is_empty() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        // Queue order uses commit time, not submission time. Concurrent submitters
        // may arrive out of order; their original observation instant stays intact.
        while state.order.front().is_some_and(|(time, _)| {
            now.saturating_duration_since(*time) >= Duration::from_secs(60)
        }) {
            let (_, key) = state.order.pop_front().expect("checked queue front");
            state.entries.remove(&key);
        }
        for sequence in sequences {
            if state.entries.len() >= self.capacity {
                break;
            }
            let key = (profile.to_owned(), *sequence);
            if state.entries.contains_key(&key) {
                continue;
            }
            state.entries.insert(key.clone(), observed);
            state.order.push_back((now, key));
        }
    }
    pub(crate) fn get(&self, profile: &str, sequence: i64) -> Option<Instant> {
        self.state
            .lock()
            .ok()?
            .entries
            .get(&(profile.to_owned(), sequence))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_is_bounded_and_duplicate_recording_does_not_refresh_source_time() {
        let tracker = ReceiptTracker::new(1);
        let time = Instant::now();
        tracker.record_at("profile", &[1], time, time);
        tracker.record_at(
            "profile",
            &[1, 2],
            time + Duration::from_secs(1),
            time + Duration::from_secs(1),
        );
        assert_eq!(tracker.get("profile", 1), Some(time));
        assert_eq!(tracker.get("profile", 2), None);
    }
    #[test]
    fn expiry_pruning_reclaims_capacity_without_refreshing_old_observations() {
        let tracker = ReceiptTracker::new(1);
        let time = Instant::now();
        tracker.record_at("profile", &[1], time, time);
        tracker.record_at(
            "profile",
            &[2],
            time + Duration::from_secs(60),
            time + Duration::from_secs(60),
        );
        assert_eq!(tracker.get("profile", 1), None);
        assert_eq!(
            tracker.get("profile", 2),
            Some(time + Duration::from_secs(60))
        );
    }
}
