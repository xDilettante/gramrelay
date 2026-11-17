use super::*;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryMessageKind {
    Regular,
    Service,
    Empty,
}

/// One history record. Debug redacts all content and incidental metadata.
pub struct HistoryMessage {
    peer: PeerId,
    raw: tl::enums::Message,
}
impl fmt::Debug for HistoryMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryMessage")
            .field("id", &self.id())
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}
impl Drop for HistoryMessage {
    fn drop(&mut self) {
        if let tl::enums::Message::Message(message) = &mut self.raw {
            message.message.zeroize();
        }
    }
}
impl HistoryMessage {
    pub fn id(&self) -> i32 {
        match &self.raw {
            tl::enums::Message::Message(message) => message.id,
            tl::enums::Message::Service(message) => message.id,
            tl::enums::Message::Empty(message) => message.id,
        }
    }
    pub fn kind(&self) -> HistoryMessageKind {
        match &self.raw {
            tl::enums::Message::Message(_) => HistoryMessageKind::Regular,
            tl::enums::Message::Service(_) => HistoryMessageKind::Service,
            tl::enums::Message::Empty(_) => HistoryMessageKind::Empty,
        }
    }
    pub fn peer(&self) -> PeerId {
        self.peer
    }
    pub fn text(&self) -> Option<&str> {
        match &self.raw {
            tl::enums::Message::Message(message) => Some(&message.message),
            _ => None,
        }
    }
    pub fn date(&self) -> Option<i32> {
        match &self.raw {
            tl::enums::Message::Message(message) => Some(message.date),
            tl::enums::Message::Service(message) => Some(message.date),
            tl::enums::Message::Empty(_) => None,
        }
    }
    pub fn sender(&self) -> Option<PeerId> {
        match &self.raw {
            tl::enums::Message::Message(message) => message.from_id.as_ref(),
            tl::enums::Message::Service(message) => message.from_id.as_ref(),
            tl::enums::Message::Empty(_) => None,
        }
        .and_then(|peer| PeerId::from_tl(peer).ok())
    }
    pub fn is_outgoing(&self) -> Option<bool> {
        match &self.raw {
            tl::enums::Message::Message(message) => Some(message.out),
            tl::enums::Message::Service(message) => Some(message.out),
            tl::enums::Message::Empty(_) => None,
        }
    }
    pub fn has_media(&self) -> bool {
        matches!(&self.raw, tl::enums::Message::Message(message) if message.media.is_some())
    }
    /// Explicit trusted Rust escape hatch for the full pinned TL record.
    /// Raw media can contain file access hashes/references: do not log, serialize
    /// or expose the raw record as an agent tool result. Prefer the safe getters.
    pub fn raw(&self) -> &tl::enums::Message {
        &self.raw
    }

    pub(super) fn checked(raw: tl::enums::Message, peer: PeerId) -> Result<Self> {
        let message = Self { raw, peer };
        if message.id() <= 0
            || message.date().is_some_and(|date| date <= 0)
            || message.text().is_some_and(|text| text.len() > 64 * 1024)
        {
            return Err(malformed());
        }
        let (found, sender) = match &message.raw {
            tl::enums::Message::Message(message) => {
                (Some(&message.peer_id), message.from_id.as_ref())
            }
            tl::enums::Message::Service(message) => {
                (Some(&message.peer_id), message.from_id.as_ref())
            }
            tl::enums::Message::Empty(message) => (message.peer_id.as_ref(), None),
        };
        if found
            .map(PeerId::from_tl)
            .transpose()?
            .is_some_and(|found| found != peer)
        {
            return Err(malformed());
        }
        if let Some(sender) = sender {
            PeerId::from_tl(sender)?;
        }
        Ok(message)
    }
}

/// A RAM-only exclusive position, bound to client, account, auth epoch and peer.
#[derive(Clone)]
pub struct HistoryCursor {
    peer: Peer,
    before: i32,
}
impl fmt::Debug for HistoryCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HistoryCursor([REDACTED])")
    }
}
impl HistoryCursor {
    fn check(&self, references: &References, peer: &Peer) -> Result<()> {
        if self.peer.check(references).is_err() || self.peer.id != peer.id || self.before <= 0 {
            return Err(error(ClientErrorKind::InvalidCursor, Outcome::NotSent));
        }
        Ok(())
    }
}

pub struct HistoryPage {
    messages: Vec<HistoryMessage>,
    next: Option<HistoryCursor>,
    total: Option<u32>,
    inexact: bool,
}
impl fmt::Debug for HistoryPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryPage")
            .field("messages", &self.messages.len())
            .field("has_next", &self.next.is_some())
            .field("inexact", &self.inexact)
            .finish_non_exhaustive()
    }
}
impl HistoryPage {
    pub fn messages(&self) -> &[HistoryMessage] {
        &self.messages
    }
    pub fn next_cursor(&self) -> Option<&HistoryCursor> {
        self.next.as_ref()
    }
    /// Advisory count for the whole history; not the number remaining.
    pub fn total_count(&self) -> Option<u32> {
        self.total
    }
    pub fn is_inexact(&self) -> bool {
        self.inexact
    }
}

impl Account {
    /// Read one bounded newest-to-oldest page. Short slices may still have more
    /// messages; use next_cursor rather than comparing length to the limit.
    pub async fn history(
        &self,
        peer: &Peer,
        limit: u16,
        before: Option<&HistoryCursor>,
    ) -> Result<HistoryPage> {
        if !(1..=100).contains(&limit) {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        if self.expected.kind != crate::AccountKind::User {
            return Err(error(
                ClientErrorKind::UnsupportedOperation,
                Outcome::NotSent,
            ));
        }
        let references = self.references();
        peer.check(&references)?;
        if let Some(cursor) = before {
            cursor.check(&references, peer)?;
        }
        let before = before.cloned();
        let peer = peer.clone();
        let byte_limit = self.client.inner.options.max_request_bytes;
        self.run(move |context, session| async move {
            peer.check(&references)?;
            if let Some(cursor) = &before {
                cursor.check(&references, &peer)?;
            }
            let offset = before.as_ref().map_or(0, |cursor| cursor.before);
            let input = input_peer(&session, peer.id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
            let request = tl::functions::messages::GetHistory {
                peer: input,
                offset_id: offset,
                offset_date: 0,
                add_offset: 0,
                limit: i32::from(limit),
                max_id: 0,
                min_id: 0,
                hash: 0,
            };
            let mut bytes = super::super::encode_request(&request, byte_limit)?;
            let response = context
                .invoke::<tl::functions::messages::GetHistory>(std::mem::take(&mut *bytes))
                .await?;
            let decoded = Decoded::new(response, peer.id.kind)?;
            if decoded.messages.len() > usize::from(limit)
                || (!decoded.inexact
                    && decoded
                        .total
                        .is_some_and(|count| (count as usize) < decoded.messages.len()))
            {
                return Err(malformed());
            }
            if decoded.messages.is_empty()
                && (decoded.inexact
                    || (offset == 0 && decoded.total.is_some_and(|count| count > 0)))
            {
                return Err(error(ClientErrorKind::IncompletePage, Outcome::Unknown));
            }
            let mut messages = Vec::with_capacity(decoded.messages.len());
            let mut previous = if offset == 0 {
                i64::MAX
            } else {
                i64::from(offset)
            };
            for raw in decoded.messages {
                let message = HistoryMessage::checked(raw, peer.id)?;
                if i64::from(message.id()) >= previous {
                    return Err(malformed());
                }
                previous = i64::from(message.id());
                messages.push(message);
            }
            let next = if decoded.complete {
                None
            } else {
                messages
                    .last()
                    .filter(|message| message.id() > 1)
                    .map(|message| HistoryCursor {
                        peer,
                        before: message.id(),
                    })
            };
            let peers = checked_entities(decoded.users, decoded.chats, references.expected)?;
            if !peers.is_empty() {
                session
                    .cache_peers(peers)
                    .await
                    .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
            }
            Ok(HistoryPage {
                messages,
                next,
                total: decoded.total,
                inexact: decoded.inexact,
            })
        })
        .await
    }
}

struct Decoded {
    messages: Vec<tl::enums::Message>,
    users: Vec<tl::enums::User>,
    chats: Vec<tl::enums::Chat>,
    complete: bool,
    inexact: bool,
    total: Option<u32>,
}
impl Decoded {
    fn new(response: tl::enums::messages::Messages, kind: PeerKind) -> Result<Self> {
        let (messages, users, chats, complete, inexact, count, position) = match response {
            tl::enums::messages::Messages::Messages(response) => (
                response.messages,
                response.users,
                response.chats,
                true,
                false,
                None,
                None,
            ),
            tl::enums::messages::Messages::Slice(response) => (
                response.messages,
                response.users,
                response.chats,
                false,
                response.inexact,
                Some(response.count),
                response.offset_id_offset,
            ),
            tl::enums::messages::Messages::ChannelMessages(response) => {
                if kind != PeerKind::Channel || response.pts < 0 {
                    return Err(malformed());
                }
                (
                    response.messages,
                    response.users,
                    response.chats,
                    false,
                    response.inexact,
                    Some(response.count),
                    response.offset_id_offset,
                )
            }
            tl::enums::messages::Messages::NotModified(_) => return Err(malformed()),
        };
        if count.is_some_and(|count| count < 0) || position.is_some_and(|position| position < 0) {
            return Err(malformed());
        }
        Ok(Self {
            messages,
            users,
            chats,
            complete,
            inexact,
            total: count.map(|count| count as u32),
        })
    }
}
