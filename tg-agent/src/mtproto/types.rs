use crate::executor::{
    EmptyArgs, ExecutorError, ExecutorErrorKind, GetOperationArgs, MessageId, ReadUpdatesArgs,
    map_only, numeric_string, valid_id,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};
use std::fmt;
pub(crate) type Result<T> = std::result::Result<T, ExecutorError>;
pub(crate) fn invalid() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidInput)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerKind {
    User,
    Chat,
    Channel,
}

/// Tagged bare ID metadata, containing no access hash and granting no authority.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerId {
    kind: PeerKind,
    id: i64,
}
impl PeerId {
    pub fn new(kind: PeerKind, id: i64) -> Result<Self> {
        let client = match kind {
            PeerKind::User => tg_mtproto::PeerId::user(id),
            PeerKind::Chat => tg_mtproto::PeerId::chat(id),
            PeerKind::Channel => tg_mtproto::PeerId::channel(id),
        };
        client.map_err(|_| invalid())?;
        Ok(Self { kind, id })
    }
    pub fn kind(self) -> PeerKind {
        self.kind
    }
    pub fn bare_id(self) -> i64 {
        self.id
    }
    pub fn as_client_id(self) -> tg_mtproto::PeerId {
        match self.kind {
            PeerKind::User => tg_mtproto::PeerId::user(self.id),
            PeerKind::Chat => tg_mtproto::PeerId::chat(self.id),
            PeerKind::Channel => tg_mtproto::PeerId::channel(self.id),
        }
        .expect("validated M2 peer identity")
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePeer {
    kind: PeerKind,
    #[serde(deserialize_with = "numeric_string")]
    id: i64,
}
impl<'de> Deserialize<'de> for PeerId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire: WirePeer =
            map_only(deserializer).map_err(|_| D::Error::custom("invalid peer"))?;
        Self::new(wire.kind, wire.id).map_err(|_| D::Error::custom("invalid peer"))
    }
}
impl Serialize for PeerId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Peer {
            kind: PeerKind,
            id: String,
        }
        Peer {
            kind: self.kind,
            id: self.id.to_string(),
        }
        .serialize(serializer)
    }
}
impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerId([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "mtproto_get_me")]
    GetMe,
    #[serde(rename = "mtproto_get_peer")]
    GetPeer,
    #[serde(rename = "mtproto_read_updates")]
    ReadUpdates,
    #[serde(rename = "mtproto_send_message")]
    SendMessage,
    #[serde(rename = "mtproto_edit_message_text")]
    EditMessageText,
    #[serde(rename = "mtproto_answer_callback_query")]
    AnswerCallbackQuery,
    #[serde(rename = "mtproto_get_operation")]
    GetOperation,
    #[serde(rename = "mtproto_send_document")]
    SendDocument,
}
impl Action {
    pub const ALL: [Self; 8] = [
        Self::GetMe,
        Self::GetPeer,
        Self::ReadUpdates,
        Self::SendMessage,
        Self::EditMessageText,
        Self::AnswerCallbackQuery,
        Self::GetOperation,
        Self::SendDocument,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::GetMe => "mtproto_get_me",
            Self::GetPeer => "mtproto_get_peer",
            Self::ReadUpdates => "mtproto_read_updates",
            Self::SendMessage => "mtproto_send_message",
            Self::EditMessageText => "mtproto_edit_message_text",
            Self::AnswerCallbackQuery => "mtproto_answer_callback_query",
            Self::GetOperation => "mtproto_get_operation",
            Self::SendDocument => "mtproto_send_document",
        }
    }
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::SendMessage
                | Self::EditMessageText
                | Self::AnswerCallbackQuery
                | Self::SendDocument
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetPeerArgs {
    pub peer: PeerId,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageArgs {
    pub peer: PeerId,
    pub text: String,
    #[serde(default)]
    pub disable_notification: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditMessageTextArgs {
    pub peer: PeerId,
    pub message_id: MessageId,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendDocumentArgs {
    pub peer: PeerId,
    pub file_handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerCallbackQueryArgs {
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub show_alert: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "tool", content = "arguments", deny_unknown_fields)]
pub enum Call {
    #[serde(rename = "mtproto_get_me")]
    GetMe(#[serde(deserialize_with = "map_only")] EmptyArgs),
    #[serde(rename = "mtproto_get_peer")]
    GetPeer(#[serde(deserialize_with = "map_only")] GetPeerArgs),
    #[serde(rename = "mtproto_read_updates")]
    ReadUpdates(#[serde(deserialize_with = "map_only")] ReadUpdatesArgs),
    #[serde(rename = "mtproto_send_message")]
    SendMessage(#[serde(deserialize_with = "map_only")] SendMessageArgs),
    #[serde(rename = "mtproto_edit_message_text")]
    EditMessageText(#[serde(deserialize_with = "map_only")] EditMessageTextArgs),
    #[serde(rename = "mtproto_answer_callback_query")]
    AnswerCallbackQuery(#[serde(deserialize_with = "map_only")] AnswerCallbackQueryArgs),
    #[serde(rename = "mtproto_get_operation")]
    GetOperation(#[serde(deserialize_with = "map_only")] GetOperationArgs),
    #[serde(rename = "mtproto_send_document")]
    SendDocument(#[serde(deserialize_with = "map_only")] SendDocumentArgs),
}
impl Call {
    pub fn action(&self) -> Action {
        match self {
            Self::GetMe(_) => Action::GetMe,
            Self::GetPeer(_) => Action::GetPeer,
            Self::ReadUpdates(_) => Action::ReadUpdates,
            Self::SendMessage(_) => Action::SendMessage,
            Self::EditMessageText(_) => Action::EditMessageText,
            Self::AnswerCallbackQuery(_) => Action::AnswerCallbackQuery,
            Self::GetOperation(_) => Action::GetOperation,
            Self::SendDocument(_) => Action::SendDocument,
        }
    }
    pub fn peer(&self) -> Option<PeerId> {
        match self {
            Self::GetPeer(v) => Some(v.peer),
            Self::SendMessage(v) => Some(v.peer),
            Self::EditMessageText(v) => Some(v.peer),
            Self::SendDocument(v) => Some(v.peer),
            _ => None,
        }
    }
    pub fn file_handle(&self) -> Option<&str> {
        match self {
            Self::SendDocument(v) => Some(&v.file_handle),
            _ => None,
        }
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::GetMe(_) | Self::GetPeer(_) => {}
            Self::ReadUpdates(v) => {
                if v.limit == 0
                    || v.limit > 100
                    || v.cursor
                        .as_ref()
                        .is_some_and(|s| s.is_empty() || s.len() > 1024)
                {
                    return Err(invalid());
                }
            }
            Self::SendMessage(v) => text(&v.text, 1, 4096)?,
            Self::EditMessageText(v) => {
                i32::try_from(v.message_id.get()).map_err(|_| invalid())?;
                text(&v.text, 1, 4096)?;
            }
            Self::SendDocument(v) => {
                valid_id(&v.file_handle)?;
                if let Some(c) = &v.caption {
                    text(c, 0, 1024)?;
                }
            }
            Self::AnswerCallbackQuery(v) => {
                valid_id(&v.reference)?;
                if let Some(t) = &v.text {
                    text(t, 0, 200)?;
                }
            }
            Self::GetOperation(v) => valid_id(&v.operation_id)?,
        }
        Ok(())
    }
}
fn text(value: &str, min: usize, max: usize) -> Result<()> {
    let len = value.encode_utf16().count();
    if len < min || len > max || value.contains('\0') {
        Err(invalid())
    } else {
        Ok(())
    }
}

/// Immutable intent data. Credentials, principal, grants and approval are absent.
#[derive(Clone, Serialize)]
pub struct Request {
    schema_version: u32,
    profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    call: Call,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    schema_version: u32,
    profile: String,
    #[serde(default, deserialize_with = "present_id")]
    operation_id: Option<String>,
    #[serde(deserialize_with = "map_only")]
    call: Call,
}
fn present_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let v: WireRequest =
            map_only(d).map_err(|_| D::Error::custom("invalid MTProto request"))?;
        if v.schema_version != 1 {
            return Err(D::Error::custom("unsupported MTProto request"));
        }
        Self::new(v.profile, v.operation_id, v.call)
            .map_err(|_| D::Error::custom("invalid MTProto request"))
    }
}
impl Request {
    pub const MAX_JSON_BYTES: usize = 65536;
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > Self::MAX_JSON_BYTES {
            return Err(invalid());
        }
        serde_json::from_slice(bytes).map_err(|_| invalid())
    }
    pub fn new(
        profile: impl Into<String>,
        operation_id: Option<String>,
        call: Call,
    ) -> Result<Self> {
        let value = Self {
            schema_version: 1,
            profile: profile.into(),
            operation_id,
            call,
        };
        valid_id(&value.profile)?;
        if value.action().is_write() != value.operation_id.is_some() {
            return Err(invalid());
        }
        if let Some(id) = &value.operation_id {
            valid_id(id)?;
        }
        value.call.validate()?;
        if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > Self::MAX_JSON_BYTES {
            return Err(invalid());
        }
        Ok(value)
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn operation_id(&self) -> Option<&str> {
        self.operation_id.as_deref()
    }
    pub fn call(&self) -> &Call {
        &self.call
    }
    pub fn action(&self) -> Action {
        self.call.action()
    }
    pub(crate) fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"tg-agent/mtproto/request/v1\0");
        hash.update(serde_json::to_vec(self).expect("validated request serialization"));
        hash.finalize().into()
    }
}
macro_rules! redacted { ($($t:ty),+)=>{$(impl fmt::Debug for $t { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str(concat!(stringify!($t),"([REDACTED])")) } })+}; }
redacted!(
    Request,
    Call,
    GetPeerArgs,
    SendMessageArgs,
    EditMessageTextArgs,
    SendDocumentArgs,
    AnswerCallbackQueryArgs
);
