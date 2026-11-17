use super::*;
use std::{collections::HashMap, ops::Range};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DialogFolder {
    Main,
    Archive,
}
impl DialogFolder {
    fn wire(self) -> i32 {
        match self {
            Self::Main => 0,
            Self::Archive => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DialogId {
    Peer(PeerId),
    Folder(DialogFolder),
    Community(PeerId),
}
pub struct Dialog {
    id: DialogId,
    peer: Option<Peer>,
    name: Option<String>,
    pinned: bool,
    unread: Option<u32>,
    top: Option<i32>,
    last: Option<Arc<HistoryMessage>>,
}
impl fmt::Debug for Dialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dialog")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl Dialog {
    pub fn id(&self) -> DialogId {
        self.id
    }
    /// Only conversations with known full routing authority have a reference.
    /// Folder/community containers are never implicit message targets.
    pub fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }
    /// Untrusted display data, intentionally available only by explicit access.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }
    pub fn unread_count(&self) -> Option<u32> {
        self.unread
    }
    pub fn top_message_id(&self) -> Option<i32> {
        self.top
    }
    pub fn last_message(&self) -> Option<&HistoryMessage> {
        self.last.as_deref()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogPageIssue {
    MissingAnchor,
    MissingPeer,
}
#[derive(Debug)]
pub enum DialogContinuation {
    More(DialogCursor),
    Complete,
    Incomplete(DialogPageIssue),
}
pub struct DialogPage {
    entries: Arc<[Dialog]>,
    range: Range<usize>,
    continuation: DialogContinuation,
}
impl fmt::Debug for DialogPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialogPage")
            .field("entries", &self.range.len())
            .field("continuation", &self.continuation)
            .finish_non_exhaustive()
    }
}
impl DialogPage {
    pub fn dialogs(&self) -> &[Dialog] {
        &self.entries[self.range.clone()]
    }
    /// Explicitly distinguish a complete scan from an unpageable partial page.
    pub fn continuation(&self) -> &DialogContinuation {
        &self.continuation
    }
}

#[derive(Clone)]
pub struct DialogCursor {
    binding: Peer,
    folder: DialogFolder,
    phase: Phase,
}
impl fmt::Debug for DialogCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DialogCursor([REDACTED])")
    }
}
#[derive(Clone)]
enum Phase {
    Pinned { entries: Arc<[Dialog]>, next: usize },
    Ordinary(Option<Anchor>),
}
#[derive(Clone)]
struct Anchor {
    peer: Peer,
    date: i32,
    message_id: i32,
}
impl Anchor {
    fn rank(&self) -> (i32, i32, i64) {
        rank(self.peer.id, self.date, self.message_id)
    }
}
fn rank(peer: PeerId, date: i32, id: i32) -> (i32, i32, i64) {
    (
        date,
        id,
        peer.backend()
            .bot_api_dialog_id()
            .expect("validated bare peer"),
    )
}
impl DialogCursor {
    fn check(&self, references: &References, folder: DialogFolder) -> Result<()> {
        if self.folder != folder || self.binding.check(references).is_err() {
            return Err(error(ClientErrorKind::InvalidCursor, Outcome::NotSent));
        }
        Ok(())
    }
}

fn pinned_page(
    entries: Arc<[Dialog]>,
    start: usize,
    limit: u16,
    binding: Peer,
    folder: DialogFolder,
) -> Result<DialogPage> {
    if start >= entries.len() {
        return Err(error(ClientErrorKind::InvalidCursor, Outcome::NotSent));
    }
    let end = (start + usize::from(limit)).min(entries.len());
    let phase = if end == entries.len() {
        Phase::Ordinary(None)
    } else {
        Phase::Pinned {
            entries: entries.clone(),
            next: end,
        }
    };
    Ok(DialogPage {
        entries,
        range: start..end,
        continuation: DialogContinuation::More(DialogCursor {
            binding,
            folder,
            phase,
        }),
    })
}

impl Account {
    /// Read one bounded user dialog page. Match all continuation variants:
    /// Incomplete preserves useful entries but never claims a completed scan.
    pub async fn dialogs(
        &self,
        folder: DialogFolder,
        limit: u16,
        cursor: Option<&DialogCursor>,
    ) -> Result<DialogPage> {
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
        if let Some(cursor) = cursor {
            cursor.check(&references, folder)?;
        }
        let cursor = cursor.cloned();
        let own_id = PeerId::user(self.expected.id.get())?;
        let byte_limit = self.client.inner.options.max_request_bytes;
        self.run(move |context, session| async move {
            if let Some(cursor) = &cursor {
                cursor.check(&references, folder)?;
            }
            let binding = references.reference(own_id);
            let phase = cursor.map(|cursor| cursor.phase);
            let anchor = match phase {
                Some(Phase::Pinned { entries, next }) => {
                    return pinned_page(entries, next, limit, binding, folder);
                }
                Some(Phase::Ordinary(anchor)) => anchor,
                None => {
                    let mut bytes = super::super::encode_request(
                        &tl::functions::messages::GetPinnedDialogs {
                            folder_id: folder.wire(),
                        },
                        byte_limit,
                    )?;
                    let tl::enums::messages::PeerDialogs::Dialogs(response) = context
                        .invoke::<tl::functions::messages::GetPinnedDialogs>(std::mem::take(
                            &mut *bytes,
                        ))
                        .await?;
                    let tl::enums::updates::State::State(state) = response.state;
                    if [
                        state.pts,
                        state.qts,
                        state.date,
                        state.seq,
                        state.unread_count,
                    ]
                    .into_iter()
                    .any(|value| value < 0)
                    {
                        return Err(malformed());
                    }
                    let entries = build(
                        Rows {
                            dialogs: response.dialogs,
                            messages: response.messages,
                            users: response.users,
                            chats: response.chats,
                        },
                        &session,
                        &references,
                        folder,
                        true,
                    )
                    .await?;
                    if !entries.is_empty() {
                        return pinned_page(entries.into(), 0, limit, binding, folder);
                    }
                    None
                }
            };
            let (offset_date, offset_id, offset_peer) = if let Some(anchor) = &anchor {
                let input = input_peer(&session, anchor.peer.id, references.expected)
                    .await?
                    .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
                (anchor.date, anchor.message_id, input)
            } else {
                (0, 0, tl::enums::InputPeer::Empty)
            };
            let request = tl::functions::messages::GetDialogs {
                exclude_pinned: true,
                folder_id: Some(folder.wire()),
                offset_date,
                offset_id,
                offset_peer,
                limit: i32::from(limit),
                hash: 0,
            };
            let mut bytes = super::super::encode_request(&request, byte_limit)?;
            let response = context
                .invoke::<tl::functions::messages::GetDialogs>(std::mem::take(&mut *bytes))
                .await?;
            let (dialogs, messages, users, chats, complete) = match response {
                tl::enums::messages::Dialogs::Dialogs(response) => (
                    response.dialogs,
                    response.messages,
                    response.users,
                    response.chats,
                    true,
                ),
                tl::enums::messages::Dialogs::Slice(response) => {
                    if response.count < 0 || (response.count as usize) < response.dialogs.len() {
                        return Err(malformed());
                    }
                    (
                        response.dialogs,
                        response.messages,
                        response.users,
                        response.chats,
                        false,
                    )
                }
                tl::enums::messages::Dialogs::NotModified(_) => return Err(malformed()),
            };
            if dialogs.len() > usize::from(limit) {
                return Err(malformed());
            }
            let entries = build(
                Rows {
                    dialogs,
                    messages,
                    users,
                    chats,
                },
                &session,
                &references,
                folder,
                false,
            )
            .await?;
            let mut previous = anchor.as_ref().map(Anchor::rank);
            for entry in &entries {
                if let DialogId::Peer(peer) = entry.id
                    && let Some(message) = entry.last_message()
                    && let Some(date) = message.date()
                {
                    let current = rank(peer, date, message.id());
                    if previous.is_some_and(|previous| current >= previous) {
                        return Err(malformed());
                    }
                    previous = Some(current);
                }
            }
            let continuation = if complete || entries.is_empty() {
                DialogContinuation::Complete
            } else {
                next_ordinary(&entries, binding, folder)
            };
            let len = entries.len();
            Ok(DialogPage {
                entries: entries.into(),
                range: 0..len,
                continuation,
            })
        })
        .await
    }
}

fn next_ordinary(entries: &[Dialog], binding: Peer, folder: DialogFolder) -> DialogContinuation {
    let Some(entry) = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.id, DialogId::Peer(_)))
    else {
        return DialogContinuation::Incomplete(DialogPageIssue::MissingAnchor);
    };
    let Some(message) = entry.last_message() else {
        return DialogContinuation::Incomplete(DialogPageIssue::MissingAnchor);
    };
    let Some(date) = message.date() else {
        return DialogContinuation::Incomplete(DialogPageIssue::MissingAnchor);
    };
    let Some(peer) = &entry.peer else {
        return DialogContinuation::Incomplete(DialogPageIssue::MissingPeer);
    };
    DialogContinuation::More(DialogCursor {
        binding,
        folder,
        phase: Phase::Ordinary(Some(Anchor {
            peer: peer.clone(),
            date,
            message_id: message.id(),
        })),
    })
}

struct Descriptor {
    id: DialogId,
    route: Option<PeerId>,
    top: Option<i32>,
    pinned: bool,
    unread: Option<u32>,
    name: Option<String>,
}
fn nonnegative(value: i32) -> Result<u32> {
    u32::try_from(value).map_err(|_| malformed())
}
fn top(value: i32) -> Result<Option<i32>> {
    nonnegative(value)?;
    Ok((value > 0).then_some(value))
}
fn descriptor(raw: tl::enums::Dialog, folder: DialogFolder, pinned: bool) -> Result<Descriptor> {
    Ok(match raw {
        tl::enums::Dialog::Dialog(raw) => {
            let route = PeerId::from_tl(&raw.peer)?;
            for value in [
                raw.read_inbox_max_id,
                raw.read_outbox_max_id,
                raw.unread_count,
                raw.unread_mentions_count,
                raw.unread_reactions_count,
                raw.unread_poll_votes_count,
            ] {
                nonnegative(value)?;
            }
            if raw.folder_id.unwrap_or(0) != folder.wire()
                || raw.pts.is_some_and(|pts| pts < 0)
                || (route.kind != PeerKind::Channel && raw.pts.is_some())
            {
                return Err(malformed());
            }
            Descriptor {
                id: DialogId::Peer(route),
                route: Some(route),
                top: top(raw.top_message)?,
                pinned: pinned || raw.pinned,
                unread: Some(raw.unread_count as u32),
                name: None,
            }
        }
        tl::enums::Dialog::Folder(raw) => {
            let tl::enums::Folder::Folder(container) = raw.folder;
            if folder != DialogFolder::Main || container.id != 1 {
                return Err(malformed());
            }
            let muted = nonnegative(raw.unread_muted_messages_count)?;
            let unmuted = nonnegative(raw.unread_unmuted_messages_count)?;
            nonnegative(raw.unread_muted_peers_count)?;
            nonnegative(raw.unread_unmuted_peers_count)?;
            Descriptor {
                id: DialogId::Folder(DialogFolder::Archive),
                route: Some(PeerId::from_tl(&raw.peer)?),
                top: top(raw.top_message)?,
                pinned: pinned || raw.pinned,
                unread: Some(muted.checked_add(unmuted).ok_or_else(malformed)?),
                name: Some(checked_name(container.title)?),
            }
        }
        tl::enums::Dialog::Community(raw) => {
            let id = PeerId::channel(raw.community_id).map_err(|_| malformed())?;
            Descriptor {
                id: DialogId::Community(id),
                route: None,
                top: None,
                pinned: pinned || raw.pinned,
                unread: None,
                name: None,
            }
        }
    })
}
fn checked_name(name: String) -> Result<String> {
    if name.len() > 4096 {
        Err(malformed())
    } else {
        Ok(name)
    }
}
fn names(users: &[tl::enums::User], chats: &[tl::enums::Chat]) -> Result<HashMap<PeerId, String>> {
    let mut names = HashMap::new();
    for user in users {
        if let tl::enums::User::User(user) = user {
            let parts = [user.first_name.as_deref(), user.last_name.as_deref()]
                .into_iter()
                .flatten()
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>();
            if !parts.is_empty() {
                if parts
                    .iter()
                    .try_fold(parts.len() - 1, |total, part| total.checked_add(part.len()))
                    .is_none_or(|size| size > 4096)
                {
                    return Err(malformed());
                }
                names.insert(
                    PeerId::user(user.id).map_err(|_| malformed())?,
                    parts.join(" "),
                );
            }
        }
    }
    for chat in chats {
        let (id, name) = match chat {
            tl::enums::Chat::Chat(chat) => (PeerId::chat(chat.id), &chat.title),
            tl::enums::Chat::Forbidden(chat) => (PeerId::chat(chat.id), &chat.title),
            tl::enums::Chat::Channel(chat) => (PeerId::channel(chat.id), &chat.title),
            tl::enums::Chat::ChannelForbidden(chat) => (PeerId::channel(chat.id), &chat.title),
            tl::enums::Chat::Community(chat) => (PeerId::channel(chat.id), &chat.title),
            tl::enums::Chat::CommunityForbidden(chat) => (PeerId::channel(chat.id), &chat.title),
            tl::enums::Chat::Empty(_) => continue,
        };
        if name.len() > 4096 {
            return Err(malformed());
        }
        names.insert(id.map_err(|_| malformed())?, name.clone());
    }
    Ok(names)
}
struct Rows {
    dialogs: Vec<tl::enums::Dialog>,
    messages: Vec<tl::enums::Message>,
    users: Vec<tl::enums::User>,
    chats: Vec<tl::enums::Chat>,
}
async fn build(
    rows: Rows,
    session: &Session,
    references: &References,
    folder: DialogFolder,
    pinned: bool,
) -> Result<Vec<Dialog>> {
    let Rows {
        dialogs: raw,
        messages,
        users,
        chats,
    } = rows;
    if raw.len() > 1024
        || messages.len() > 1024
        || users
            .len()
            .checked_add(chats.len())
            .is_none_or(|count| count > 1024)
    {
        return Err(malformed());
    }
    let names = names(&users, &chats)?;
    let peers = checked_entities(users, chats, references.expected)?;
    let mut descriptors = Vec::with_capacity(raw.len());
    let mut seen = HashSet::new();
    let mut targets: HashMap<i32, HashSet<PeerId>> = HashMap::new();
    for raw in raw {
        let item = descriptor(raw, folder, pinned)?;
        if !seen.insert(item.id) {
            return Err(malformed());
        }
        if let (Some(peer), Some(id)) = (item.route, item.top) {
            targets.entry(id).or_default().insert(peer);
        }
        descriptors.push(item);
    }
    let mut by_message = HashMap::new();
    for raw in messages {
        let peer = match &raw {
            tl::enums::Message::Message(raw) => PeerId::from_tl(&raw.peer_id)?,
            tl::enums::Message::Service(raw) => PeerId::from_tl(&raw.peer_id)?,
            tl::enums::Message::Empty(raw) => match &raw.peer_id {
                Some(peer) => PeerId::from_tl(peer)?,
                None => {
                    let matches = targets
                        .get(&raw.id)
                        .filter(|peers| peers.len() == 1)
                        .ok_or_else(malformed)?;
                    *matches.iter().next().ok_or_else(malformed)?
                }
            },
        };
        let message = HistoryMessage::checked(raw, peer)?;
        if by_message
            .insert((peer, message.id()), Arc::new(message))
            .is_some()
        {
            return Err(malformed());
        }
    }
    let mut entries = Vec::with_capacity(descriptors.len());
    for item in descriptors {
        let last = match (item.route, item.top) {
            (Some(peer), Some(id)) => {
                Some(by_message.get(&(peer, id)).cloned().ok_or_else(malformed)?)
            }
            _ => None,
        };
        let name_peer = match item.id {
            DialogId::Peer(peer) | DialogId::Community(peer) => Some(peer),
            DialogId::Folder(_) => None,
        };
        let name = item
            .name
            .or_else(|| name_peer.and_then(|peer| names.get(&peer).cloned()));
        entries.push(Dialog {
            id: item.id,
            peer: None,
            name,
            pinned: item.pinned,
            unread: item.unread,
            top: item.top,
            last,
        });
    }
    if !peers.is_empty() {
        session
            .cache_peers(peers)
            .await
            .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
    }
    for entry in &mut entries {
        if let DialogId::Peer(id) = entry.id
            && input_peer(session, id, references.expected)
                .await?
                .is_some()
        {
            entry.peer = Some(references.reference(id));
        }
    }
    Ok(entries)
}
