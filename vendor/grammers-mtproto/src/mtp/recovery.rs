//! Tracks requests to recover server bodies separately from application RPCs.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::{Deserialization, RecoveryTarget};
use crate::MsgId;

pub(super) const RECOVERY_LIMIT: usize = 128;
const HISTORY_LIMIT: usize = 1024;
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(10);
// Cover both ends of the server ID freshness window and its second granularity.
const HISTORY_AGE: Duration = Duration::from_secs(331);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct MissingResponse {
    pub answer: i64,
    pub target: RecoveryTarget,
}

impl MissingResponse {
    fn is_received(
        self,
        received: &impl Fn(i64) -> bool,
        answered: &impl Fn(MsgId) -> bool,
    ) -> bool {
        match self.target {
            RecoveryTarget::Unsolicited => received(self.answer),
            RecoveryTarget::Request(id) | RecoveryTarget::SaltRefresh(id) => answered(id),
        }
    }

    fn unavailable(self) -> Deserialization {
        Deserialization::ResponseUnavailable {
            target: self.target,
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Queued,
    Sent(MsgId),
    Resolved,
}

struct Entry {
    body: MissingResponse,
    noticed_at: Instant,
    state: State,
}

impl Entry {
    fn is_expired(&self, now: Instant) -> bool {
        matches!(self.state, State::Resolved)
            && now.saturating_duration_since(self.noticed_at) > HISTORY_AGE
    }
}

#[derive(Default)]
pub(super) struct Recoveries {
    entries: Vec<Entry>,
    notices_in_frame: Vec<MissingResponse>,
    refusals_in_frame: Vec<MsgId>,
    overflow_in_frame: HashSet<MissingResponse>,
    packet: Option<(i64, MsgId)>,
    pub previous_packet_was_recovery: bool,
}

impl Recoveries {
    pub fn notice(&mut self, notice: MissingResponse) {
        let now = Instant::now();
        let mut active = self.notices_in_frame.len();
        let mut retained = active;
        for entry in self.entries.iter().filter(|entry| !entry.is_expired(now)) {
            if entry.body.answer == notice.answer {
                return;
            }
            active += usize::from(!matches!(entry.state, State::Resolved));
            retained += 1;
        }
        if self
            .notices_in_frame
            .iter()
            .any(|seen| seen.answer == notice.answer)
        {
            return;
        }
        if active >= RECOVERY_LIMIT || retained >= HISTORY_LIMIT {
            self.overflow_in_frame.insert(notice);
        } else {
            self.notices_in_frame.push(notice);
        }
    }

    pub fn is_sent_query(&self, query: MsgId) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.state, State::Sent(id) if id == query))
    }

    pub fn refuse_query(&mut self, query: MsgId) {
        if !self.refusals_in_frame.contains(&query) {
            self.refusals_in_frame.push(query);
        }
    }

    pub fn commit_frame(
        &mut self,
        received: impl Fn(i64) -> bool,
        answered: impl Fn(MsgId) -> bool,
        session_recreated: bool,
        results: &mut Vec<Deserialization>,
    ) {
        if self.entries.is_empty()
            && self.notices_in_frame.is_empty()
            && self.overflow_in_frame.is_empty()
        {
            return;
        }
        let now = Instant::now();
        self.prune(now);
        for entry in &mut self.entries {
            if matches!(entry.state, State::Resolved) {
                continue;
            }
            if entry.body.is_received(&received, &answered) {
                entry.state = State::Resolved;
            } else if session_recreated
                || received(entry.body.answer)
                || matches!(entry.state, State::Sent(id) if self.refusals_in_frame.contains(&id))
            {
                Self::resolve_unavailable(entry, results);
            }
        }
        for notice in self.notices_in_frame.drain(..) {
            let state = if notice.is_received(&received, &answered) {
                State::Resolved
            } else if session_recreated || received(notice.answer) {
                results.push(notice.unavailable());
                State::Resolved
            } else {
                State::Queued
            };
            self.entries.push(Entry {
                body: notice,
                noticed_at: now,
                state,
            });
        }
        let mut unavailable = HashSet::new();
        for notice in self.overflow_in_frame.drain() {
            if !notice.is_received(&received, &answered) && unavailable.insert(notice.target) {
                results.push(notice.unavailable());
            }
        }
        self.refusals_in_frame.clear();
    }

    pub fn discard_frame(&mut self) {
        self.notices_in_frame.clear();
        self.refusals_in_frame.clear();
        self.overflow_in_frame.clear();
    }

    pub fn pending_answer(&self) -> Option<i64> {
        self.entries
            .iter()
            .find(|entry| matches!(entry.state, State::Queued))
            .map(|entry| entry.body.answer)
    }

    pub fn stage_packet(&mut self, answer: i64, query: MsgId) {
        // The transport commits its framing when it packs this packet. Once
        // packed, finishing its write is safe even if the body arrives first.
        self.packet = Some((answer, query));
    }

    pub fn packet_is_staged(&self) -> bool {
        self.packet.is_some()
    }

    pub fn discard_packet(&mut self) {
        self.packet = None;
    }

    pub fn packet_written(&mut self) {
        let packet = self.packet.take();
        self.previous_packet_was_recovery = packet.is_some();
        if let Some((answer, query)) = packet
            && let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| entry.body.answer == answer)
            && matches!(entry.state, State::Queued)
        {
            entry.state = State::Sent(query);
        }
    }

    pub fn poll_timeouts(
        &mut self,
        now: Instant,
        results: &mut Vec<Deserialization>,
    ) -> Option<Instant> {
        let mut next = None;
        for entry in &mut self.entries {
            if matches!(entry.state, State::Resolved) {
                continue;
            }
            let deadline = entry.noticed_at + RECOVERY_TIMEOUT;
            if deadline <= now {
                Self::resolve_unavailable(entry, results);
            } else {
                next = Some(next.map_or(deadline, |previous: Instant| previous.min(deadline)));
            }
        }
        self.prune(now);
        next
    }

    fn resolve_unavailable(entry: &mut Entry, results: &mut Vec<Deserialization>) {
        entry.state = State::Resolved;
        results.push(entry.body.unavailable());
    }

    fn prune(&mut self, now: Instant) {
        self.entries.retain(|entry| !entry.is_expired(now));
    }
}
