use crate::{
    session::updates::{GapReason, GapScope, MAX_BATCH_BYTES},
    tl,
};
use grammers_session::{
    types::UpdatesState,
    updates::{MessageBox, MessageBoxes, UpdateAndPeers, UpdatesLike},
};
use std::time::Instant;
use tl::{Deserializable, Serializable};
#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;

pub(super) const MAX_EVENTS: usize = 4096;
const MAX_ENTITIES: usize = 1024;
const MAX_CHANNELS: usize = 10000;
pub(super) type Gap = (GapScope, GapReason);
type Result<T> = std::result::Result<T, Gap>;
fn malformed() -> Gap {
    (GapScope::Account, GapReason::Malformed)
}
fn capacity() -> Gap {
    (GapScope::Account, GapReason::BacklogLimit)
}
fn quiet<T>(work: impl FnOnce() -> T) -> T {
    tracing::dispatcher::with_default(&tracing::Dispatch::none(), work)
}
fn sdk<T>(work: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| quiet(work))).map_err(|_| malformed())
}

pub(super) enum Work {
    Difference(tl::functions::updates::GetDifference),
    Channel(tl::functions::updates::GetChannelDifference),
    Socket(Instant),
}
pub(super) struct Packet {
    pub events: Vec<tl::enums::Update>,
    pub users: Vec<tl::enums::User>,
    pub chats: Vec<tl::enums::Chat>,
}
impl Packet {
    pub fn empty() -> Self {
        Self {
            events: vec![],
            users: vec![],
            chats: vec![],
        }
    }
    fn from_tuple((events, users, chats): UpdateAndPeers) -> Self {
        Self {
            events: events.into_iter().map(|(event, _)| event).collect(),
            users,
            chats,
        }
    }
    pub fn encode(&self) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        super::super::encode_request(self, MAX_BATCH_BYTES).map_err(|_| capacity())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(capacity());
        }
        if bytes.is_empty() {
            return Ok(Self::empty());
        }
        let mut cursor = tl::deserialize::Cursor::from_slice(bytes);
        if u32::deserialize(&mut cursor).map_err(|_| malformed())? != 0x54475531 {
            return Err(malformed());
        }
        let packet = Self {
            events: Vec::<tl::enums::Update>::deserialize(&mut cursor).map_err(|_| malformed())?,
            users: Vec::<tl::enums::User>::deserialize(&mut cursor).map_err(|_| malformed())?,
            chats: Vec::<tl::enums::Chat>::deserialize(&mut cursor).map_err(|_| malformed())?,
        };
        if cursor.pos() != bytes.len() {
            return Err(malformed());
        }
        limits(packet.events.len(), packet.users.len(), packet.chats.len())?;
        Ok(packet)
    }
}
impl Serializable for Packet {
    fn serialize(&self, sink: &mut impl FnMut(&[u8])) {
        0x54475531u32.serialize(sink);
        self.events.serialize(sink);
        self.users.serialize(sink);
        self.chats.serialize(sink);
    }
}
pub(super) struct Machine {
    boxes: MessageBoxes,
    retained_bytes: usize,
}
impl Machine {
    pub fn state(&self) -> UpdatesState {
        self.boxes.session_state()
    }
    pub fn load(state: UpdatesState) -> Result<Self> {
        valid_state(&state)?;
        let boxes = sdk(|| {
            // `load` interprets zero as absent. An acknowledged zero is known:
            // set_state retains it, and SDK controls initiate catch-up explicitly.
            let mut boxes = MessageBoxes::new();
            boxes.set_state(tl::types::updates::State {
                pts: state.pts,
                qts: state.qts,
                date: state.date,
                seq: state.seq,
                unread_count: 0,
            });
            for channel in state.channels {
                boxes.try_set_channel_state(channel.id, channel.pts);
                let _ = boxes.process_updates(UpdatesLike::Updates(
                    tl::types::UpdateShort {
                        update: tl::types::UpdateChannelTooLong {
                            channel_id: channel.id,
                            pts: None,
                        }
                        .into(),
                        date: 0,
                    }
                    .into(),
                ));
            }
            let _ = boxes.process_updates(UpdatesLike::ConnectionClosed);
            boxes
        })?;
        Ok(Self {
            boxes,
            retained_bytes: 0,
        })
    }
    pub fn work(&mut self) -> Work {
        quiet(|| {
            let deadline = self.boxes.check_deadlines();
            if let Some(mut request) = self.boxes.get_difference() {
                request.pts_limit = Some(MAX_EVENTS as i32);
                request.qts_limit = Some(MAX_EVENTS as i32);
                request.pts_total_limit = Some(MAX_EVENTS as i32);
                Work::Difference(request)
            } else if let Some(request) = self.boxes.get_channel_difference() {
                Work::Channel(request)
            } else {
                Work::Socket(deadline)
            }
        })
    }
    pub fn socket(&mut self, input: UpdatesLike) -> Result<Packet> {
        if matches!(input, UpdatesLike::MalformedUpdates) {
            return Err(malformed());
        }
        let before = self.state();
        let short_sent = matches!(&input, UpdatesLike::ShortSentMessage { .. });
        let normalized = sdk(|| MessageBoxes::normalize_updates(input))?;
        let mut normalized = match normalized {
            Ok(updates) => updates,
            Err(_) => {
                let _ = sdk(|| self.boxes.process_updates(UpdatesLike::ConnectionClosed))?;
                return self.finish(Packet::empty(), &before);
            }
        };
        // The SDK's short-sent adapter emits NewMessage even for channel sends.
        // Reuse its complete message construction; fix only this known envelope.
        if short_sent {
            for update in &mut normalized.updates {
                if let tl::enums::Update::NewMessage(value) = update
                    && matches!(&value.message,tl::enums::Message::Message(message) if matches!(message.peer_id,tl::enums::Peer::Channel(_)))
                {
                    let replacement = tl::types::UpdateNewChannelMessage {
                        message: value.message.clone(),
                        pts: value.pts,
                        pts_count: value.pts_count,
                    };
                    *update = replacement.into();
                }
            }
        }
        limits(
            normalized.updates.len(),
            normalized.users.len(),
            normalized.chats.len(),
        )?;
        if normalized.date < 0
            || normalized.seq < 0
            || normalized.seq == i32::MAX
            || normalized.seq_start < 0
            || normalized.seq_start > normalized.seq
        {
            return Err(malformed());
        }
        self.charge(encoded_size(&normalized)?)?;
        sanitize_updates(&mut normalized.updates, &before)?;
        let output = sdk(|| {
            self.boxes
                .process_updates(UpdatesLike::Updates(normalized.into()))
        })?;
        self.finish(
            output
                .map(Packet::from_tuple)
                .unwrap_or_else(|_| Packet::empty()),
            &before,
        )
    }
    pub fn difference(&mut self, mut input: tl::enums::updates::Difference) -> Result<Packet> {
        if matches!(input, tl::enums::updates::Difference::TooLong(_)) {
            return Err((GapScope::Account, GapReason::AccountTooLong));
        }
        let before = self.state();
        self.charge(encoded_size(&input)?)?;
        match &mut input {
            tl::enums::updates::Difference::Difference(value) => {
                limits(
                    value.new_messages.len()
                        + value.new_encrypted_messages.len()
                        + value.other_updates.len(),
                    value.users.len(),
                    value.chats.len(),
                )?;
                sanitize_updates(&mut value.other_updates, &before)?;
            }
            tl::enums::updates::Difference::Slice(value) => {
                limits(
                    value.new_messages.len()
                        + value.new_encrypted_messages.len()
                        + value.other_updates.len(),
                    value.users.len(),
                    value.chats.len(),
                )?;
                sanitize_updates(&mut value.other_updates, &before)?;
            }
            _ => {}
        }
        let output = sdk(|| self.boxes.apply_difference(input))?;
        self.finish(Packet::from_tuple(output), &before)
    }
    pub fn channel(
        &mut self,
        channel: i64,
        mut input: tl::enums::updates::ChannelDifference,
    ) -> Result<Packet> {
        if matches!(input, tl::enums::updates::ChannelDifference::TooLong(_)) {
            return Err((GapScope::Channel(channel), GapReason::ChannelTooLong));
        }
        let before = self.state();
        self.charge(encoded_size(&input)?)?;
        if let tl::enums::updates::ChannelDifference::Difference(value) = &mut input {
            limits(
                value.new_messages.len() + value.other_updates.len(),
                value.users.len(),
                value.chats.len(),
            )?;
            sanitize_updates(&mut value.other_updates, &before)?;
        }
        let output = sdk(|| self.boxes.apply_channel_difference(input))?;
        self.finish(Packet::from_tuple(output), &before)
    }
    fn charge(&mut self, count: usize) -> Result<()> {
        self.retained_bytes = self
            .retained_bytes
            .checked_add(count)
            .filter(|bytes| *bytes <= MAX_BATCH_BYTES)
            .ok_or_else(capacity)?;
        Ok(())
    }
    fn finish(&mut self, packet: Packet, before: &UpdatesState) -> Result<Packet> {
        if self.boxes.buffered_updates() > MAX_EVENTS
            || self.boxes.buffered_updates_bytes(MAX_BATCH_BYTES) > MAX_BATCH_BYTES
        {
            return Err(capacity());
        }
        let state = self.state();
        valid_state(&state)?;
        let channels: std::collections::HashMap<_, _> = state
            .channels
            .iter()
            .map(|channel| (channel.id, channel.pts))
            .collect();
        if state.pts < before.pts
            || state.qts < before.qts
            || state.seq < before.seq
            || state.date < before.date
            || before
                .channels
                .iter()
                .any(|old| channels.get(&old.id).is_none_or(|pts| *pts < old.pts))
        {
            return Err(malformed());
        }
        limits(packet.events.len(), packet.users.len(), packet.chats.len())?;
        packet.encode()?;
        if self.boxes.buffered_updates() == 0 {
            self.retained_bytes = 0;
        }
        Ok(packet)
    }
}
pub(super) fn checked_state(state: UpdatesState) -> Result<UpdatesState> {
    valid_state(&state)?;
    Ok(state)
}
fn valid_state(state: &UpdatesState) -> Result<()> {
    if [state.pts, state.qts, state.date, state.seq]
        .iter()
        .any(|value| *value < 0 || *value == i32::MAX)
        || state.channels.iter().any(|channel| {
            channel.pts < 0
                || channel.pts == i32::MAX
                || grammers_session::types::PeerId::channel(channel.id).is_none()
        })
    {
        return Err(malformed());
    }
    if state.channels.len() > MAX_CHANNELS {
        return Err(capacity());
    }
    Ok(())
}
fn limits(events: usize, users: usize, chats: usize) -> Result<()> {
    if events > MAX_EVENTS || users.saturating_add(chats) > MAX_ENTITIES {
        Err(capacity())
    } else {
        Ok(())
    }
}
fn encoded_size(value: &impl Serializable) -> Result<usize> {
    let mut size = 0usize;
    value.serialize(&mut |part| size = size.saturating_add(part.len()));
    if size > MAX_BATCH_BYTES {
        Err(capacity())
    } else {
        Ok(size)
    }
}
fn sanitize_updates(updates: &mut [tl::enums::Update], state: &UpdatesState) -> Result<()> {
    let channels: std::collections::HashMap<_, _> = state
        .channels
        .iter()
        .map(|channel| (channel.id, channel.pts))
        .collect();
    for update in updates {
        if let tl::enums::Update::ChannelTooLong(value) = update {
            if grammers_session::types::PeerId::channel(value.channel_id).is_none() {
                return Err(malformed());
            }
            if !channels.contains_key(&value.channel_id) {
                return Err((
                    GapScope::Channel(value.channel_id),
                    GapReason::MissingBaseline,
                ));
            }
            value.pts = None;
        }
        if let Some((metadata, count)) = sdk(|| MessageBox::update_metadata(update))? {
            let pts = metadata.pts();
            if pts < 0 || pts == i32::MAX || count < 0 {
                return Err(malformed());
            }
            // Zero means that ordering is intentionally absent, but malformed
            // negative counts must still be rejected before the SDK filter.
            if pts == 0 {
                continue;
            }
            if pts < count {
                return Err(malformed());
            }
            let local = match metadata {
                MessageBox::Common { .. } => state.pts,
                MessageBox::Secondary { .. } => state.qts,
                MessageBox::Channel { channel_id, .. } => {
                    if grammers_session::types::PeerId::channel(channel_id).is_none() {
                        return Err(malformed());
                    }
                    channels
                        .get(&channel_id)
                        .copied()
                        .ok_or((GapScope::Channel(channel_id), GapReason::MissingBaseline))?
                }
            };
            if local.checked_add(count).is_none() {
                return Err(malformed());
            }
        }
    }
    Ok(())
}
