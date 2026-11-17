//! Durable whole-batch boundary. The M2 receiver is integrated separately.
#![cfg_attr(not(test), allow(dead_code))]
use super::{
    AccountIdentity, Session, SessionErrorKind, SessionOptions,
    data::{Snapshot, validate_updates},
    types::{Result, error, storage},
};
use grammers_session::types::{PeerId, UpdatesState};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use zeroize::Zeroize;

pub(crate) const MAX_BATCH_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateId {
    serial: u64,
    nonce: [u8; 16],
}
impl UpdateId {
    fn valid(self, serial: u64) -> bool {
        self.serial != 0 && self.serial <= serial && self.nonce != [0; 16]
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BatchKind {
    Baseline,
    Events,
    Resync { gap: UpdateId },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateDraft {
    pub account: AccountIdentity,
    pub kind: BatchKind,
    #[serde(deserialize_with = "required_option")]
    pub base: Option<UpdatesState>,
    pub checkpoint: UpdatesState,
    pub payload: UpdatePayload,
}
/// Own the decoded bytes from the first array element, including error paths.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct UpdatePayload(Vec<u8>);
impl From<Vec<u8>> for UpdatePayload {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}
impl std::ops::Deref for UpdatePayload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}
impl UpdatePayload {
    fn capacity(&self) -> usize {
        self.0.capacity()
    }
}
#[cfg(test)]
std::thread_local! { static PAYLOAD_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
impl Drop for UpdatePayload {
    fn drop(&mut self) {
        self.0.zeroize();
        #[cfg(test)]
        PAYLOAD_DROPS.with(|drops| {
            assert!(self.0.is_empty());
            drops.set(drops.get() + 1);
        });
    }
}
impl<'de> Deserialize<'de> for UpdatePayload {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = UpdatePayload;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("bounded update bytes")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                // One bounded allocation avoids leaving private data in an old
                // allocation during Vec growth. The guard also owns partial data.
                let mut payload = UpdatePayload(Vec::with_capacity(MAX_BATCH_BYTES));
                while let Some(byte) = sequence.next_element::<u8>()? {
                    if payload.0.len() == MAX_BATCH_BYTES {
                        return Err(serde::de::Error::custom("update bytes limit"));
                    }
                    payload.0.push(byte);
                }
                Ok(payload)
            }
        }
        decoder.deserialize_seq(Bytes)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingUpdates {
    pub id: UpdateId,
    pub draft: UpdateDraft,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GapScope {
    Account,
    Channel(i64),
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GapReason {
    AccountTooLong,
    ChannelTooLong,
    MissingHash,
    MissingBaseline,
    Malformed,
    QueueOverflow,
    BacklogLimit,
    ChannelAccessLost,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateGap {
    pub id: UpdateId,
    pub scope: GapScope,
    pub reason: GapReason,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateJournal {
    serial: u64,
    #[serde(deserialize_with = "required_option")]
    pub pending: Option<PendingUpdates>,
    #[serde(deserialize_with = "required_option")]
    last_ack: Option<UpdateId>,
    #[serde(deserialize_with = "required_option")]
    pub gap: Option<UpdateGap>,
}
pub(super) fn required_option<'de, D: serde::Deserializer<'de>, T: DeserializeOwned>(
    decoder: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(decoder)
}

impl UpdateJournal {
    fn next_id(&mut self) -> Result<UpdateId> {
        let serial = self
            .serial
            .checked_add(1)
            .ok_or_else(|| error(SessionErrorKind::Capacity))?;
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(storage)?;
        if nonce == [0; 16] {
            return Err(error(SessionErrorKind::Storage));
        }
        self.serial = serial;
        Ok(UpdateId { serial, nonce })
    }
    pub(super) fn clear_authorization(&mut self) {
        // Never reuse serials after logout/local credential discard.
        self.pending = None;
        self.last_ack = None;
        self.gap = None;
    }
    pub(super) fn validate(&self, snapshot: &Snapshot, options: &SessionOptions) -> Result<()> {
        let invalid = || error(SessionErrorKind::Authentication);
        if let Some(last) = self.last_ack
            && (!last.valid(self.serial) || snapshot.acknowledged.is_none())
        {
            return Err(invalid());
        }
        if let Some(gap) = self.gap {
            if !gap.id.valid(self.serial)
                || self
                    .last_ack
                    .is_some_and(|last| last.serial >= gap.id.serial)
            {
                return Err(invalid());
            }
            if let GapScope::Channel(id) = gap.scope
                && PeerId::channel(id).is_none()
            {
                return Err(invalid());
            }
            if snapshot.account.is_none() {
                return Err(invalid());
            }
        }
        if let Some(pending) = &self.pending {
            if !pending.id.valid(self.serial)
                || self
                    .last_ack
                    .is_some_and(|last| last.serial >= pending.id.serial)
                || self
                    .gap
                    .is_some_and(|gap| gap.id.serial == pending.id.serial)
            {
                return Err(invalid());
            }
            if self.gap.is_some_and(|gap| match pending.draft.kind {
                BatchKind::Events => pending.id.serial > gap.id.serial,
                BatchKind::Resync { .. } => pending.id.serial < gap.id.serial,
                BatchKind::Baseline => true,
            }) {
                return Err(invalid());
            }
            pending.draft.validate(options).map_err(|_| invalid())?;
            check_draft(snapshot, &pending.draft).map_err(|_| invalid())?;
        }
        Ok(())
    }
}
impl UpdateDraft {
    fn validate(&self, options: &SessionOptions) -> Result<()> {
        // Decoders choose their own allocation growth. Persisted bounds concern
        // lengths; owned caller capacity is checked separately before admission.
        if self.payload.len() > MAX_BATCH_BYTES {
            return Err(error(SessionErrorKind::Capacity));
        }
        validate_updates(&self.checkpoint, options)?;
        if let Some(base) = &self.base {
            validate_updates(base, options)?;
            monotone(base, &self.checkpoint)?;
        }
        match self.kind {
            BatchKind::Baseline if self.base.is_some() || !self.payload.is_empty() => {
                return Err(error(SessionErrorKind::InvalidInput));
            }
            BatchKind::Events if self.base.is_none() => {
                return Err(error(SessionErrorKind::InvalidInput));
            }
            BatchKind::Resync { .. } if !self.payload.is_empty() => {
                return Err(error(SessionErrorKind::InvalidInput));
            }
            _ => {}
        }
        Ok(())
    }
}
fn monotone(base: &UpdatesState, next: &UpdatesState) -> Result<()> {
    if next.pts < base.pts || next.qts < base.qts || next.date < base.date || next.seq < base.seq {
        return Err(error(SessionErrorKind::InvalidInput));
    }
    let next: std::collections::HashMap<_, _> = next
        .channels
        .iter()
        .map(|channel| (channel.id, channel.pts))
        .collect();
    if base
        .channels
        .iter()
        .any(|channel| next.get(&channel.id).is_none_or(|pts| *pts < channel.pts))
    {
        return Err(error(SessionErrorKind::InvalidInput));
    }
    Ok(())
}
fn account(snapshot: &Snapshot, expected: AccountIdentity) -> Result<()> {
    if snapshot.quarantined {
        return Err(error(SessionErrorKind::Quarantined));
    }
    if snapshot.account != Some(expected) {
        return Err(error(SessionErrorKind::InvalidInput));
    }
    Ok(())
}
fn check_draft(snapshot: &Snapshot, draft: &UpdateDraft) -> Result<()> {
    // Snapshot validation also calls this while quarantined; pending data must
    // remain readable by the codec until explicit local/remote logout cleanup.
    if snapshot.account != Some(draft.account) || snapshot.acknowledged != draft.base {
        return Err(error(SessionErrorKind::InvalidInput));
    }
    match draft.kind {
        BatchKind::Resync { gap }
            if snapshot.updates.gap.is_none_or(|current| current.id != gap) =>
        {
            Err(error(SessionErrorKind::InvalidInput))
        }
        BatchKind::Baseline if snapshot.updates.gap.is_some() => {
            Err(error(SessionErrorKind::InvalidInput))
        }
        _ => Ok(()),
    }
}

pub(crate) struct UpdatesView {
    pub acknowledged: Option<UpdatesState>,
    pub pending: Option<PendingUpdates>,
    pub gap: Option<UpdateGap>,
}
impl Session {
    pub(crate) fn updates_view(&self, expected: AccountIdentity) -> Result<UpdatesView> {
        self.inner.database.shared.read(|view| {
            account(&view.snapshot, expected)?;
            Ok(UpdatesView {
                acknowledged: view.snapshot.acknowledged.clone(),
                pending: view.snapshot.updates.pending.clone(),
                gap: view.snapshot.updates.gap,
            })
        })?
    }
    pub(crate) async fn stage_updates(&self, draft: UpdateDraft) -> Result<PendingUpdates> {
        if draft.payload.capacity() > MAX_BATCH_BYTES
            || draft.checkpoint.channels.capacity() > self.inner.database.options.max_channels
            || draft.base.as_ref().is_some_and(|base| {
                base.channels.capacity() > self.inner.database.options.max_channels
            })
        {
            return Err(error(SessionErrorKind::Capacity));
        }
        draft.validate(&self.inner.database.options)?;
        self.inner
            .database
            .call(move |native| {
                let mut result = None;
                native.mutate(|snapshot| {
                    account(snapshot, draft.account)?;
                    check_draft(snapshot, &draft)?;
                    if snapshot.updates.pending.is_some() {
                        return Err(error(SessionErrorKind::Owned));
                    }
                    if snapshot.updates.gap.is_some()
                        && !matches!(draft.kind, BatchKind::Resync { .. })
                    {
                        return Err(error(SessionErrorKind::InvalidInput));
                    }
                    let id = snapshot.updates.next_id()?;
                    let pending = PendingUpdates { id, draft };
                    result = Some(pending.clone());
                    snapshot.updates.pending = Some(pending);
                    Ok(())
                })?;
                result.ok_or_else(|| error(SessionErrorKind::Storage))
            })
            .await
    }
    pub(crate) async fn acknowledge_updates(
        &self,
        expected: AccountIdentity,
        id: UpdateId,
    ) -> Result<()> {
        self.inner
            .database
            .call(move |native| {
                native.mutate(|snapshot| {
                    account(snapshot, expected)?;
                    if snapshot.updates.last_ack == Some(id) {
                        return Ok(());
                    }
                    if snapshot
                        .updates
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.id != id)
                    {
                        return Err(error(SessionErrorKind::InvalidInput));
                    }
                    let pending = snapshot
                        .updates
                        .pending
                        .take()
                        .ok_or_else(|| error(SessionErrorKind::Storage))?;
                    check_draft(snapshot, &pending.draft)?;
                    if let BatchKind::Resync { .. } = pending.draft.kind {
                        snapshot.updates.gap = None;
                    }
                    snapshot.acknowledged = Some(pending.draft.checkpoint.clone());
                    snapshot.updates.last_ack = Some(id);
                    Ok(())
                })
            })
            .await
    }
    pub(crate) async fn record_update_gap(
        &self,
        expected: AccountIdentity,
        scope: GapScope,
        reason: GapReason,
    ) -> Result<UpdateGap> {
        if let GapScope::Channel(id) = scope
            && PeerId::channel(id).is_none()
        {
            return Err(error(SessionErrorKind::InvalidInput));
        }
        self.inner
            .database
            .call(move |native| {
                let mut result = None;
                native.mutate(|snapshot| {
                    account(snapshot, expected)?;
                    let gap = UpdateGap {
                        id: snapshot.updates.next_id()?,
                        scope,
                        reason,
                    };
                    if snapshot
                        .updates
                        .pending
                        .as_ref()
                        .is_some_and(|pending| !matches!(pending.draft.kind, BatchKind::Events))
                    {
                        snapshot.updates.pending = None;
                    }
                    snapshot.updates.gap = Some(gap);
                    result = Some(gap);
                    Ok(())
                })?;
                result.ok_or_else(|| error(SessionErrorKind::Storage))
            })
            .await
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    #[test]
    fn partial_payload_and_failed_outer_struct_use_the_wiping_owner() {
        for input in [
            r#"[112,114,105,118,97,116,101,"bad"]"#,
            r#"[112,114,105,118,97,116,101,256]"#,
        ] {
            let before = PAYLOAD_DROPS.with(|drops| drops.get());
            assert!(serde_json::from_str::<UpdatePayload>(input).is_err());
            assert_eq!(PAYLOAD_DROPS.with(|drops| drops.get()), before + 1);
        }
        let before = PAYLOAD_DROPS.with(|drops| drops.get());
        assert!(
            serde_json::from_str::<UpdateDraft>(
                r#"{"payload":[112,114,105,118,97,116,101],"unknown":true}"#
            )
            .is_err()
        );
        assert_eq!(PAYLOAD_DROPS.with(|drops| drops.get()), before + 1);
        let before = PAYLOAD_DROPS.with(|drops| drops.get());
        let sequence = serde::de::value::SeqDeserializer::<_, serde::de::value::Error>::new(
            (0..=MAX_BATCH_BYTES).map(|_| 1u8),
        );
        assert!(UpdatePayload::deserialize(sequence).is_err());
        assert_eq!(PAYLOAD_DROPS.with(|drops| drops.get()), before + 1);
    }
}
