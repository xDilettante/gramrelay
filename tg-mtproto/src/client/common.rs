use super::{
    Client, ClientErrorKind, Core, Outcome, auth,
    error::{Result, error},
    runtime::Context,
};
use crate::{AccountIdentity, Session};
use grammers_session::types::{PeerId as BackendPeerId, PeerInfo};
use grammers_tl_types as tl;
use std::{
    collections::HashSet,
    fmt,
    future::Future,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
mod dialogs;
mod history;
pub use dialogs::{
    Dialog, DialogContinuation, DialogCursor, DialogFolder, DialogId, DialogPage, DialogPageIssue,
};
mod text;
pub use history::{HistoryCursor, HistoryMessage, HistoryMessageKind, HistoryPage};
pub use text::{MessageReceipt, SendText};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PeerKind {
    User,
    Chat,
    Channel,
}

/// Intentional public metadata, containing no reusable access hash.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PeerId {
    kind: PeerKind,
    id: i64,
}
impl PeerId {
    pub fn user(id: i64) -> Result<Self> {
        Self::new(PeerKind::User, id, BackendPeerId::user(id))
    }
    pub fn chat(id: i64) -> Result<Self> {
        Self::new(PeerKind::Chat, id, BackendPeerId::chat(id))
    }
    pub fn channel(id: i64) -> Result<Self> {
        Self::new(PeerKind::Channel, id, BackendPeerId::channel(id))
    }
    fn new(kind: PeerKind, id: i64, checked: Option<BackendPeerId>) -> Result<Self> {
        checked
            .map(|_| Self { kind, id })
            .ok_or_else(|| error(ClientErrorKind::InvalidInput, Outcome::NotSent))
    }
    pub fn kind(self) -> PeerKind {
        self.kind
    }
    pub fn bare_id(self) -> i64 {
        self.id
    }
    fn backend(self) -> BackendPeerId {
        match self.kind {
            PeerKind::User => BackendPeerId::user(self.id),
            PeerKind::Chat => BackendPeerId::chat(self.id),
            PeerKind::Channel => BackendPeerId::channel(self.id),
        }
        .expect("PeerId constructors enforce the backend range")
    }
    fn from_info(info: &PeerInfo) -> Result<Self> {
        match info {
            PeerInfo::User { id, .. } => Self::user(*id),
            PeerInfo::Chat { id } => Self::chat(*id),
            PeerInfo::Channel { id, .. } => Self::channel(*id),
        }
        .map_err(|_| malformed())
    }
    fn from_tl(peer: &tl::enums::Peer) -> Result<Self> {
        match peer {
            tl::enums::Peer::User(peer) => Self::user(peer.user_id),
            tl::enums::Peer::Chat(peer) => Self::chat(peer.chat_id),
            tl::enums::Peer::Channel(peer) => Self::channel(peer.channel_id),
        }
        .map_err(|_| malformed())
    }
}

/// A lazy host facade. Each action verifies live account identity.
#[derive(Clone)]
pub struct Account {
    client: Client,
    expected: AccountIdentity,
}
impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account([REDACTED])")
    }
}

/// A RAM reference valid only for its originating client/account/auth epoch.
/// Access hashes remain in the encrypted Session and never in this handle.
#[derive(Clone)]
pub struct Peer {
    owner: Weak<Core>,
    expected: AccountIdentity,
    epoch: u64,
    id: PeerId,
}
impl fmt::Debug for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Peer")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl Peer {
    pub fn id(&self) -> PeerId {
        self.id
    }
    fn check(&self, binding: &References) -> Result<()> {
        if !Weak::ptr_eq(&self.owner, &binding.owner)
            || self.expected != binding.expected
            || self.epoch != binding.epoch.load(Ordering::Acquire)
        {
            return Err(error(ClientErrorKind::InvalidPeer, Outcome::NotSent));
        }
        Ok(())
    }
}

// Owned jobs must not retain a Client/Core: dropping the last host Client still
// closes admission and wakes shutdown while an action is pending.
struct References {
    owner: Weak<Core>,
    expected: AccountIdentity,
    epoch: Arc<AtomicU64>,
}
impl References {
    fn reference(&self, id: PeerId) -> Peer {
        Peer {
            owner: self.owner.clone(),
            expected: self.expected,
            epoch: self.epoch.load(Ordering::Acquire),
            id,
        }
    }
}

impl Client {
    /// Construct a lazy facade; this performs no I/O or identity verification.
    pub fn account(&self, expected: AccountIdentity) -> Account {
        Account {
            client: self.clone(),
            expected,
        }
    }
}
impl Account {
    pub fn identity(&self) -> AccountIdentity {
        self.expected
    }
    async fn run<T, F, Fut>(&self, action: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Context, Session) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        auth::check_stored(&self.client.inner.session, self.expected)?;
        let reservation = self.client.reserve()?;
        let session = self.client.inner.session.clone();
        let authorization = self.client.inner.authorization.clone();
        let expected = self.expected;
        let limit = self.client.inner.options.max_request_bytes;
        self.client
            .submit(reservation, move |context| async move {
                context.hold_authorization_read(authorization).await;
                auth::verify_in_context(&context, &session, expected, limit).await?;
                action(context, session).await
            })
            .await
    }
    fn references(&self) -> References {
        References {
            owner: Arc::downgrade(&self.client.inner),
            expected: self.expected,
            epoch: self.client.inner.auth_epoch.clone(),
        }
    }
    /// Resolve a Telegram username and durably cache only peer routing metadata.
    pub async fn resolve_username(&self, username: &str) -> Result<Peer> {
        let username = username.strip_prefix('@').unwrap_or(username);
        if username.is_empty()
            || username.len() > 32
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let references = self.references();
        let username = username.to_owned();
        let limit = self.client.inner.options.max_request_bytes;
        self.run(move |context, session| async move {
            let request = tl::functions::contacts::ResolveUsername {
                username,
                referer: None,
            };
            let mut bytes = super::encode_request(&request, limit)?;
            let tl::enums::contacts::ResolvedPeer::Peer(response) = context
                .invoke::<tl::functions::contacts::ResolveUsername>(std::mem::take(&mut *bytes))
                .await?;
            let id = PeerId::from_tl(&response.peer)?;
            let peers = checked_entities(response.users, response.chats, references.expected)?;
            session
                .cache_peers(peers)
                .await
                .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
            input_peer(&session, id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::Unknown))?;
            Ok(references.reference(id))
        })
        .await
    }
    /// Read cached routing metadata after live self verification. No target RPC.
    pub async fn cached_peer(&self, id: PeerId) -> Result<Option<Peer>> {
        let references = self.references();
        self.run(move |_, session| async move {
            Ok(input_peer(&session, id, references.expected)
                .await?
                .map(|_| references.reference(id)))
        })
        .await
    }
}

fn malformed() -> super::ClientError {
    error(ClientErrorKind::Decode, Outcome::Unknown)
}

fn checked_entities(
    users: Vec<tl::enums::User>,
    chats: Vec<tl::enums::Chat>,
    expected: AccountIdentity,
) -> Result<Vec<PeerInfo>> {
    let count = users
        .len()
        .checked_add(chats.len())
        .filter(|count| *count <= 1024)
        .ok_or_else(malformed)?;
    let mut seen = HashSet::with_capacity(count);
    let mut peers = Vec::with_capacity(count);
    for user in users {
        let peer = PeerInfo::from(&user);
        let id = PeerId::from_info(&peer)?;
        if !seen.insert(id) {
            return Err(malformed());
        }
        if let tl::enums::User::User(user) = &user
            && ((user.is_self && user.id != expected.id.get())
                || (user.id == expected.id.get()
                    && (!user.is_self || user.bot != (expected.kind == crate::AccountKind::Bot))))
        {
            return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
        }
        // Only the dedicated live self check may overwrite our identity record.
        if id != PeerId::user(expected.id.get())? {
            peers.push(peer);
        }
    }
    for chat in chats {
        let peer = PeerInfo::from(&chat);
        if !seen.insert(PeerId::from_info(&peer)?) {
            return Err(malformed());
        }
        if !matches!(chat, tl::enums::Chat::Empty(_)) {
            peers.push(peer);
        }
    }
    Ok(peers)
}

async fn input_peer(
    session: &Session,
    id: PeerId,
    expected: AccountIdentity,
) -> Result<Option<tl::enums::InputPeer>> {
    if id == PeerId::user(expected.id.get())? {
        return Ok(Some(tl::enums::InputPeer::PeerSelf));
    }
    let peer = session
        .cached_peer(id.backend())
        .await
        .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
    Ok(match peer {
        Some(PeerInfo::User {
            id,
            auth: Some(auth),
            ..
        }) => Some(tl::enums::InputPeer::User(tl::types::InputPeerUser {
            user_id: id,
            access_hash: auth.hash(),
        })),
        Some(PeerInfo::Chat { id }) => Some(tl::enums::InputPeer::Chat(tl::types::InputPeerChat {
            chat_id: id,
        })),
        Some(PeerInfo::Channel {
            id,
            auth: Some(auth),
            ..
        }) => Some(tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
            channel_id: id,
            access_hash: auth.hash(),
        })),
        _ => None,
    })
}
