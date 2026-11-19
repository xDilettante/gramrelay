use super::*;
use crate::executor::MessageId;
use base64::{Engine, engine::general_purpose::STANDARD};
use tg_mtproto::tl::{self, Serializable};
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    pub schema_version: u8,
    pub event: Event,
}
pub(super) struct Encoded {
    pub payload: Vec<u8>,
    pub query: Option<i64>,
    pub hash: [u8; 32],
}
#[derive(Clone, Copy)]
pub(super) struct Stamp {
    pub source: [u8; 32],
    pub ordinal: u64,
    pub id: [u8; 32],
    pub kind: &'static str,
}
pub(super) fn identity(value: tg_mtproto::AccountIdentity) -> Result<ProfileIdentity> {
    match value.kind {
        tg_mtproto::AccountKind::Bot => ProfileIdentity::mtproto_bot(value.id.get()),
        tg_mtproto::AccountKind::User => ProfileIdentity::mtproto_user(value.id.get()),
        _ => Err(invalid()),
    }
}
pub(super) fn peer(value: &tl::enums::Peer) -> Result<PeerId> {
    let (kind, id) = match value {
        tl::enums::Peer::User(v) => (PeerKind::User, v.user_id),
        tl::enums::Peer::Chat(v) => (PeerKind::Chat, v.chat_id),
        tl::enums::Peer::Channel(v) => (PeerKind::Channel, v.channel_id),
    };
    PeerId::new(kind, id).map_err(|_| invalid())
}
fn message_id(id: i32) -> Result<MessageId> {
    MessageId::new(i64::from(id)).map_err(|_| invalid())
}
fn constructor(value: &impl Serializable) -> u32 {
    let mut prefix = [0u8; 4];
    let mut used = 0;
    value.serialize(&mut |part| {
        let count = (4 - used).min(part.len());
        prefix[used..used + count].copy_from_slice(&part[..count]);
        used += count;
    });
    u32::from_le_bytes(prefix)
}
fn message(value: &tl::enums::Message, edited: bool, limit: usize) -> Result<Event> {
    match value {
        tl::enums::Message::Message(v) => {
            if v.message.len() > limit {
                return Err(capacity());
            }
            if v.date < 0 {
                return Err(invalid());
            }
            Ok(Event::Message {
                peer: peer(&v.peer_id)?,
                message_id: message_id(v.id)?,
                sender: v.from_id.as_ref().map(peer).transpose()?,
                date: v.date,
                text: v.message.clone(),
                outgoing: v.out,
                edited,
                has_media: v.media.is_some(),
            })
        }
        tl::enums::Message::Service(v) => {
            if v.date < 0 {
                return Err(invalid());
            }
            Ok(Event::Service {
                peer: peer(&v.peer_id)?,
                message_id: message_id(v.id)?,
                sender: v.from_id.as_ref().map(peer).transpose()?,
                date: v.date,
                action: constructor(&v.action),
            })
        }
        tl::enums::Message::Empty(v) => Ok(Event::Empty {
            peer: v.peer_id.as_ref().map(peer).transpose()?,
            message_id: message_id(v.id)?,
        }),
    }
}
fn ids(values: &[i32]) -> Result<Vec<MessageId>> {
    if values.len() > 4096 {
        return Err(capacity());
    }
    values.iter().copied().map(message_id).collect()
}
fn callback(
    query: i64,
    actor: i64,
    primary: Option<PeerId>,
    msg: Option<i32>,
    data: &Option<Vec<u8>>,
    game: &Option<String>,
    limit: usize,
) -> Result<(Event, Option<i64>)> {
    if data
        .as_ref()
        .is_some_and(|v| v.len().saturating_mul(4) / 3 > limit)
        || game.as_ref().is_some_and(|v| v.len() > limit)
    {
        return Err(capacity());
    }
    Ok((
        Event::Callback {
            peer: primary,
            message_id: msg.map(message_id).transpose()?,
            actor: PeerId::new(PeerKind::User, actor).map_err(|_| invalid())?,
            data_base64: data.as_ref().map(|v| STANDARD.encode(v)),
            game: game.clone(),
        },
        Some(query),
    ))
}
pub(super) fn update(value: &tl::enums::Update, limit: usize) -> Result<(Event, Option<i64>)> {
    use tl::enums::Update as U;
    let event = match value {
        U::NewMessage(v) => message(&v.message, false, limit)?,
        U::NewChannelMessage(v) => message(&v.message, false, limit)?,
        U::EditMessage(v) => message(&v.message, true, limit)?,
        U::EditChannelMessage(v) => message(&v.message, true, limit)?,
        U::DeleteMessages(v) => Event::Deleted {
            peer: None,
            message_ids: ids(&v.messages)?,
        },
        U::DeleteChannelMessages(v) => Event::Deleted {
            peer: Some(PeerId::new(PeerKind::Channel, v.channel_id).map_err(|_| invalid())?),
            message_ids: ids(&v.messages)?,
        },
        U::ReadHistoryInbox(v) => Event::ReadPosition {
            peer: peer(&v.peer)?,
            max_id: MessagePosition::new(v.max_id)?,
            outgoing: false,
        },
        U::ReadHistoryOutbox(v) => Event::ReadPosition {
            peer: peer(&v.peer)?,
            max_id: MessagePosition::new(v.max_id)?,
            outgoing: true,
        },
        U::ReadChannelInbox(v) => Event::ReadPosition {
            peer: PeerId::new(PeerKind::Channel, v.channel_id).map_err(|_| invalid())?,
            max_id: MessagePosition::new(v.max_id)?,
            outgoing: false,
        },
        U::ReadChannelOutbox(v) => Event::ReadPosition {
            peer: PeerId::new(PeerKind::Channel, v.channel_id).map_err(|_| invalid())?,
            max_id: MessagePosition::new(v.max_id)?,
            outgoing: true,
        },
        U::BotCallbackQuery(v) => {
            return callback(
                v.query_id,
                v.user_id,
                Some(peer(&v.peer)?),
                Some(v.msg_id),
                &v.data,
                &v.game_short_name,
                limit,
            );
        }
        U::InlineBotCallbackQuery(v) => {
            return callback(
                v.query_id,
                v.user_id,
                None,
                None,
                &v.data,
                &v.game_short_name,
                limit,
            );
        }
        _ => Event::Unsupported {
            constructor: constructor(value),
        },
    };
    Ok((event, None))
}
pub(super) fn encode(event: Event, query: Option<i64>, options: &Options) -> Result<Encoded> {
    let limit = options
        .max_event_bytes
        .checked_sub(query.map_or(0, |_| 8))
        .ok_or_else(capacity)?;
    let payload = crate::journal::types::canonical_json(
        &Payload {
            schema_version: 1,
            event,
        },
        limit,
    )
    .map_err(|_| capacity())?;
    let hash = digest(&payload, query);
    Ok(Encoded {
        payload,
        query,
        hash,
    })
}
pub(super) fn digest(payload: &[u8], query: Option<i64>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"tg-agent/mtproto/record/v1\0");
    hash.update(payload);
    hash.update([u8::from(query.is_some())]);
    if let Some(q) = query {
        hash.update(q.to_le_bytes());
    }
    hash.finalize().into()
}
pub(super) fn batch(
    value: &tg_mtproto::UpdateBatch,
    options: &Options,
) -> Result<(Stamp, Vec<Encoded>)> {
    use tg_mtproto::UpdateBatchKind as K;
    let (kind, empty) = match value.kind() {
        K::Baseline => ("baseline", Event::Baseline),
        K::Events => ("events", Event::Checkpoint),
        K::Resync => ("resync", Event::ResyncPrepared),
    };
    let mut events = Vec::new();
    let mut total = 0usize;
    if value.updates().is_empty() {
        events.push(encode(empty, None, options)?);
    } else {
        if value.updates().len() > 4096 {
            return Err(capacity());
        }
        for raw in value.updates() {
            let (event, query) = update(raw, options.max_event_bytes)?;
            let value = encode(event, query, options)?;
            total = total
                .checked_add(value.payload.len() + value.query.map_or(0, |_| 8))
                .ok_or_else(capacity)?;
            if total > options.max_batch_bytes {
                return Err(capacity());
            }
            events.push(value);
        }
    }
    Ok((
        Stamp {
            source: value.source_id(),
            ordinal: value.ordinal(),
            id: value.id(),
            kind,
        },
        events,
    ))
}
pub(super) fn gap(
    value: &tg_mtproto::ResyncRequired,
    options: &Options,
) -> Result<(Stamp, Vec<Encoded>)> {
    use tg_mtproto::{UpdateGapReason as R, UpdateScope as S};
    let peer = match value.scope() {
        S::Account => None,
        S::Channel(id) => Some(PeerId::new(PeerKind::Channel, id).map_err(|_| invalid())?),
        _ => return Err(invalid()),
    };
    let reason = match value.reason() {
        R::AccountTooLong => "account_too_long",
        R::ChannelTooLong => "channel_too_long",
        R::MissingHash => "missing_hash",
        R::MissingBaseline => "missing_baseline",
        R::Malformed => "malformed",
        R::QueueOverflow => "queue_overflow",
        R::BacklogLimit => "backlog_limit",
        R::ChannelAccessLost => "channel_access_lost",
        _ => return Err(invalid()),
    };
    Ok((
        Stamp {
            source: value.source_id(),
            ordinal: value.ordinal(),
            id: value.id(),
            kind: "gap",
        },
        vec![encode(
            Event::Gap {
                peer,
                reason: reason.into(),
            },
            None,
            options,
        )?],
    ))
}
