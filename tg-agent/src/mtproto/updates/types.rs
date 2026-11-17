use super::*;
use crate::executor::MessageId;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr, sync::Arc, time::Instant};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Cursor {
    pub(super) profile: String,
    pub(super) sequence: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorWire {
    profile: String,
    sequence: i64,
}
impl From<Cursor> for String {
    fn from(c: Cursor) -> Self {
        format!(
            "tgmt1.{}",
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&CursorWire {
                    profile: c.profile,
                    sequence: c.sequence
                })
                .expect("cursor metadata serialization")
            )
        )
    }
}
impl FromStr for Cursor {
    type Err = JournalError;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() > 512 {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(value.strip_prefix("tgmt1.").ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let value: CursorWire = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        valid_id(&value.profile)?;
        if value.sequence < 0 {
            return Err(invalid());
        }
        Ok(Self {
            profile: value.profile,
            sequence: value.sequence,
        })
    }
}
impl TryFrom<String> for Cursor {
    type Error = JournalError;
    fn try_from(s: String) -> Result<Self> {
        s.parse()
    }
}
impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from(self.clone()))
    }
}

/// Deliberately limited public projection. Text/bytes and actor IDs are data,
/// not authorization or callback/file capabilities.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Event {
    Message {
        peer: PeerId,
        message_id: MessageId,
        sender: Option<PeerId>,
        date: i32,
        text: String,
        outgoing: bool,
        edited: bool,
        has_media: bool,
    },
    Service {
        peer: PeerId,
        message_id: MessageId,
        sender: Option<PeerId>,
        date: i32,
        action: u32,
    },
    Empty {
        peer: Option<PeerId>,
        message_id: MessageId,
    },
    Deleted {
        peer: Option<PeerId>,
        message_ids: Vec<MessageId>,
    },
    ReadPosition {
        peer: PeerId,
        max_id: MessagePosition,
        outgoing: bool,
    },
    Callback {
        peer: Option<PeerId>,
        message_id: Option<MessageId>,
        actor: PeerId,
        data_base64: Option<String>,
        game: Option<String>,
    },
    Unsupported {
        constructor: u32,
    },
    Baseline,
    Checkpoint,
    ResyncPrepared,
    Gap {
        peer: Option<PeerId>,
        reason: String,
    },
    SourceChanged,
}
impl Event {
    /// Only the reviewed primary peer determines scope. Sender/actor never does.
    pub fn peer(&self) -> Option<PeerId> {
        match self {
            Self::Message { peer, .. }
            | Self::Service { peer, .. }
            | Self::ReadPosition { peer, .. } => Some(*peer),
            Self::Empty { peer, .. }
            | Self::Deleted { peer, .. }
            | Self::Callback { peer, .. }
            | Self::Gap { peer, .. } => *peer,
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MessagePosition(i32);
impl MessagePosition {
    pub fn get(self) -> i32 {
        self.0
    }
    pub(super) fn new(id: i32) -> Result<Self> {
        if id < 0 { Err(invalid()) } else { Ok(Self(id)) }
    }
}
impl TryFrom<String> for MessagePosition {
    type Error = JournalError;
    fn try_from(s: String) -> Result<Self> {
        let id: i32 = s.parse().map_err(|_| invalid())?;
        if id.to_string() != s {
            return Err(invalid());
        }
        Self::new(id)
    }
}
impl From<MessagePosition> for String {
    fn from(v: MessagePosition) -> String {
        v.0.to_string()
    }
}

#[derive(Clone, Serialize)]
pub struct StoredEvent {
    pub(super) cursor: Cursor,
    pub(super) event: Event,
    pub(super) received_at_ms: i64,
    pub(super) checkpoint_acknowledged: bool,
    #[serde(skip)]
    pub(crate) callback_query: Option<i64>,
    #[serde(skip)]
    pub(crate) origin: Arc<str>,
    #[serde(skip)]
    pub(crate) callback_received: Option<Instant>,
}
impl StoredEvent {
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }
    pub fn event(&self) -> &Event {
        &self.event
    }
    pub fn received_at_ms(&self) -> i64 {
        self.received_at_ms
    }
    pub fn checkpoint_acknowledged(&self) -> bool {
        self.checkpoint_acknowledged
    }
    /// Same-journal provenance only; it does not grant permission to act.
    pub fn is_from(&self, journal: &Journal) -> bool {
        Arc::ptr_eq(&self.origin, &journal.origin_token())
    }
    /// Private query metadata exists; this does not issue an action reference.
    pub fn has_callback_query(&self) -> bool {
        self.callback_query.is_some()
    }
    /// First local collection observation, lost on journal restart. This does
    /// not establish Telegram-side age or guarantee server acceptance.
    pub fn callback_observed_at(&self) -> Option<Instant> {
        self.callback_received
    }
}
pub struct Page {
    pub(super) events: Vec<StoredEvent>,
    pub(super) cursor: Cursor,
    pub(super) has_more: bool,
}
impl Page {
    pub fn events(&self) -> &[StoredEvent] {
        &self.events
    }
    pub fn into_events(self) -> Vec<StoredEvent> {
        self.events
    }
    pub fn next_cursor(&self) -> &Cursor {
        &self.cursor
    }
    pub fn has_more(&self) -> bool {
        self.has_more
    }
}
#[derive(Clone)]
pub struct SourceLease {
    pub(super) profile: String,
    pub(super) account: ProfileIdentity,
    pub(super) revision: i64,
    pub(super) issuer: String,
    pub(super) until_ms: i64,
    pub(super) deadline: Instant,
}
impl SourceLease {
    pub fn expires_at_ms(&self) -> i64 {
        self.until_ms
    }
}
#[derive(Clone)]
pub struct Receipt {
    pub(super) inserted: bool,
    pub(super) start: i64,
    pub(super) end: i64,
}
impl Receipt {
    pub fn inserted(&self) -> bool {
        self.inserted
    }
    pub fn first_position(&self) -> i64 {
        self.start
    }
    pub fn last_position(&self) -> i64 {
        self.end
    }
}
pub enum Ingested {
    Idle,
    Batch(Receipt),
    Gap(tg_mtproto::ResyncRequired),
}
pub struct IngestError {
    pub(super) journal: Option<JournalError>,
    pub(super) client: Option<tg_mtproto::ClientError>,
}
impl IngestError {
    pub fn journal_error(&self) -> Option<&JournalError> {
        self.journal.as_ref()
    }
    pub fn client_error(&self) -> Option<&tg_mtproto::ClientError> {
        self.client.as_ref()
    }
}
impl From<JournalError> for IngestError {
    fn from(e: JournalError) -> Self {
        Self {
            journal: Some(e),
            client: None,
        }
    }
}
impl From<tg_mtproto::ClientError> for IngestError {
    fn from(e: tg_mtproto::ClientError) -> Self {
        Self {
            journal: None,
            client: Some(e),
        }
    }
}
impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("mtproto_ingestion")
    }
}
impl std::error::Error for IngestError {}
macro_rules! redacted {($($ty:ty),+)=>{$(impl fmt::Debug for $ty {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str(concat!(stringify!($ty),"([REDACTED])"))}})+};}
redacted!(
    Cursor,
    Event,
    MessagePosition,
    StoredEvent,
    Page,
    SourceLease,
    Receipt,
    Ingested,
    IngestError
);
