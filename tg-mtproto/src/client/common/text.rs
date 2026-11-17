use super::*;
use zeroize::{Zeroize, Zeroizing};

/// An immutable send intent. Never automatically replay an Unknown result,
/// even when its random_id has been retained in a host journal.
pub struct SendText {
    text: Zeroizing<String>,
    random_id: i64,
    silent: bool,
}
impl fmt::Debug for SendText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SendText([REDACTED])")
    }
}
impl SendText {
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let text = checked_text(text)?;
        let random_id = new_random_id()?;
        Ok(Self {
            text,
            random_id,
            silent: false,
        })
    }
    /// Restore an existing host intent. This does not authorize retrying Unknown.
    pub fn with_random_id(text: impl Into<String>, random_id: i64) -> Result<Self> {
        let text = checked_text(text)?;
        if random_id == 0 {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(Self {
            text,
            random_id,
            silent: false,
        })
    }
    pub fn random_id(&self) -> i64 {
        self.random_id
    }
    pub fn silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }
}
pub(super) fn new_random_id() -> Result<i64> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|_| error(ClientErrorKind::Backend, Outcome::NotSent))?;
    let id = i64::from_ne_bytes(bytes);
    Ok(if id == 0 { 1 } else { id })
}
fn checked_text(text: impl Into<String>) -> Result<Zeroizing<String>> {
    let text = Zeroizing::new(text.into());
    if text.is_empty()
        || text.len() > 16384
        || text.contains('\0')
        || text.encode_utf16().count() > 4096
    {
        return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
    }
    Ok(text)
}

/// Confirmed routing metadata, containing no message content or access hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageReceipt {
    peer: PeerId,
    message_id: i32,
    date: i32,
}
impl MessageReceipt {
    pub fn peer(self) -> PeerId {
        self.peer
    }
    pub fn message_id(self) -> i32 {
        self.message_id
    }
    pub fn date(self) -> i32 {
        self.date
    }
}
struct SendRequest(tl::functions::messages::SendMessage);
impl Drop for SendRequest {
    fn drop(&mut self) {
        self.0.message.zeroize();
    }
}
struct EditRequest(tl::functions::messages::EditMessage);
impl Drop for EditRequest {
    fn drop(&mut self) {
        if let Some(text) = &mut self.0.message {
            text.zeroize();
        }
    }
}

impl Account {
    /// Send one immutable text intent. Unknown outcomes are never replayed.
    pub async fn send_text(&self, peer: &Peer, intent: SendText) -> Result<MessageReceipt> {
        self.send_text_inner(peer, intent, None).await
    }
    /// Cap the entire owned action, including identity/queue waits. An expired
    /// cap refuses dispatch; a timeout after dispatch remains Unknown.
    pub async fn send_text_until(
        &self,
        peer: &Peer,
        intent: SendText,
        deadline: std::time::Instant,
    ) -> Result<MessageReceipt> {
        self.send_text_inner(peer, intent, Some(deadline)).await
    }
    async fn send_text_inner(
        &self,
        peer: &Peer,
        intent: SendText,
        deadline: Option<std::time::Instant>,
    ) -> Result<MessageReceipt> {
        let references = self.references();
        peer.check(&references)?;
        let peer = peer.clone();
        let limit = self.client.inner.options.max_request_bytes;
        self.run_until(deadline, move |context, session| async move {
            peer.check(&references)?;
            let input = input_peer(&session, peer.id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
            let mut bytes = {
                let request = SendRequest(tl::functions::messages::SendMessage {
                    no_webpage: true,
                    silent: intent.silent,
                    background: false,
                    clear_draft: false,
                    noforwards: false,
                    update_stickersets_order: false,
                    invert_media: false,
                    allow_paid_floodskip: false,
                    peer: input,
                    reply_to: None,
                    message: intent.text.to_string(),
                    random_id: intent.random_id,
                    reply_markup: None,
                    entities: None,
                    schedule_date: None,
                    schedule_repeat_period: None,
                    send_as: None,
                    quick_reply_shortcut: None,
                    effect: None,
                    allow_paid_stars: None,
                    suggested_post: None,
                    rich_message: None,
                });
                super::super::encode_request(&request.0, limit)?
            };
            let response = context
                .invoke::<tl::functions::messages::SendMessage>(std::mem::take(&mut *bytes))
                .await?;
            match response {
                tl::enums::Updates::UpdateShortSentMessage(sent) => short_receipt(peer.id, &sent),
                response => {
                    let updates = unpack(response, &session, references.expected).await?;
                    let id = sent_id(&updates, intent.random_id)?;
                    matching_message(&updates, peer.id, id, &intent.text, false)
                }
            }
        })
        .await
    }

    /// Edit one positive message ID. No late/ambiguous edit is replayed.
    pub async fn edit_text(
        &self,
        peer: &Peer,
        message_id: i32,
        text: impl Into<String>,
    ) -> Result<MessageReceipt> {
        self.edit_text_inner(peer, message_id, text, None).await
    }
    /// The same edit with an absolute cap covering the whole owned action.
    pub async fn edit_text_until(
        &self,
        peer: &Peer,
        message_id: i32,
        text: impl Into<String>,
        deadline: std::time::Instant,
    ) -> Result<MessageReceipt> {
        self.edit_text_inner(peer, message_id, text, Some(deadline))
            .await
    }
    async fn edit_text_inner(
        &self,
        peer: &Peer,
        message_id: i32,
        text: impl Into<String>,
        deadline: Option<std::time::Instant>,
    ) -> Result<MessageReceipt> {
        let text = checked_text(text)?;
        if message_id <= 0 {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let references = self.references();
        peer.check(&references)?;
        let peer = peer.clone();
        let limit = self.client.inner.options.max_request_bytes;
        self.run_until(deadline, move |context, session| async move {
            peer.check(&references)?;
            let input = input_peer(&session, peer.id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
            let mut bytes = {
                let request = EditRequest(tl::functions::messages::EditMessage {
                    no_webpage: true,
                    invert_media: false,
                    peer: input,
                    id: message_id,
                    message: Some(text.to_string()),
                    media: None,
                    reply_markup: None,
                    entities: None,
                    schedule_date: None,
                    schedule_repeat_period: None,
                    quick_reply_shortcut_id: None,
                    rich_message: None,
                });
                super::super::encode_request(&request.0, limit)?
            };
            let response = context
                .invoke::<tl::functions::messages::EditMessage>(std::mem::take(&mut *bytes))
                .await?;
            let updates = unpack(response, &session, references.expected).await?;
            matching_message(&updates, peer.id, message_id, &text, true)
        })
        .await
    }
}

pub(super) async fn unpack(
    response: tl::enums::Updates,
    session: &Session,
    expected: AccountIdentity,
) -> Result<Vec<tl::enums::Update>> {
    let (updates, users, chats) = match response {
        tl::enums::Updates::Updates(response) => (response.updates, response.users, response.chats),
        tl::enums::Updates::Combined(response) => {
            (response.updates, response.users, response.chats)
        }
        tl::enums::Updates::UpdateShort(response) => (vec![response.update], vec![], vec![]),
        _ => return Err(malformed()),
    };
    if updates.len() > 1024 {
        return Err(malformed());
    }
    let peers = checked_entities(users, chats, expected)?;
    session
        .cache_peers(peers)
        .await
        .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
    Ok(updates)
}

pub(super) fn matching_message(
    updates: &[tl::enums::Update],
    peer: PeerId,
    id: i32,
    text: &str,
    edit: bool,
) -> Result<MessageReceipt> {
    let mut receipt = None;
    for update in updates {
        let (raw, pts, pts_count) = match (edit, update) {
            (false, tl::enums::Update::NewMessage(update)) => {
                (&update.message, update.pts, update.pts_count)
            }
            (false, tl::enums::Update::NewChannelMessage(update)) => {
                (&update.message, update.pts, update.pts_count)
            }
            (true, tl::enums::Update::EditMessage(update)) => {
                (&update.message, update.pts, update.pts_count)
            }
            (true, tl::enums::Update::EditChannelMessage(update)) => {
                (&update.message, update.pts, update.pts_count)
            }
            _ => continue,
        };
        let message = match raw {
            tl::enums::Message::Message(message) => message,
            tl::enums::Message::Service(message) => {
                if message.id == id && PeerId::from_tl(&message.peer_id)? == peer {
                    return Err(malformed());
                }
                continue;
            }
            tl::enums::Message::Empty(message) => {
                if message.id == id
                    && message
                        .peer_id
                        .as_ref()
                        .map(PeerId::from_tl)
                        .transpose()?
                        .is_none_or(|found| found == peer)
                {
                    return Err(malformed());
                }
                continue;
            }
        };
        if message.id != id || PeerId::from_tl(&message.peer_id)? != peer {
            continue;
        }
        if receipt.is_some()
            || (!edit && !message.out)
            || message.message != text
            || message.date <= 0
            || pts < 0
            || pts_count < 0
        {
            return Err(malformed());
        }
        receipt = Some(MessageReceipt {
            peer,
            message_id: id,
            date: message.date,
        });
    }
    receipt.ok_or_else(malformed)
}
pub(super) fn short_receipt(
    peer: PeerId,
    sent: &tl::types::UpdateShortSentMessage,
) -> Result<MessageReceipt> {
    if !sent.out || sent.id <= 0 || sent.date <= 0 || sent.pts < 0 || sent.pts_count < 0 {
        return Err(malformed());
    }
    Ok(MessageReceipt {
        peer,
        message_id: sent.id,
        date: sent.date,
    })
}
pub(super) fn sent_id(updates: &[tl::enums::Update], random_id: i64) -> Result<i32> {
    let mut ids = updates.iter().filter_map(|update| match update {
        tl::enums::Update::MessageId(value) if value.random_id == random_id => Some(value.id),
        _ => None,
    });
    let id = ids.next().filter(|id| *id > 0).ok_or_else(malformed)?;
    if ids.next().is_some() {
        return Err(malformed());
    }
    Ok(id)
}
