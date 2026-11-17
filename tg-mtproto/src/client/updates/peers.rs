use super::*;
use grammers_session::types::PeerInfo;

// Routing metadata must survive even when sequencing rejects an envelope:
// otherwise a new private channel can become impossible to resynchronize.
pub(super) async fn apply<T, F>(
    context: &runtime::Context,
    session: &Session,
    expected: AccountIdentity,
    mut machine: Machine,
    input: T,
    work: F,
) -> Result<(Machine, std::result::Result<Packet, engine::Gap>)>
where
    T: Metadata + Send + 'static,
    F: FnOnce(&mut Machine, T) -> std::result::Result<Packet, engine::Gap> + Send + 'static,
{
    let (machine, result, peers) = context
        .compute(move || {
            let peers = metadata(&input, expected);
            let (result, peers) = match peers {
                Ok(peers) => (work(&mut machine, input), peers),
                Err(reason) => (Err(reason), vec![]),
            };
            Ok((machine, result, peers))
        })
        .await?;
    if !peers.is_empty() {
        session.cache_peers(peers).await.map_err(stored)?;
    }
    context.check_active()?;
    Ok((machine, result))
}
fn metadata(
    value: &impl Metadata,
    expected: AccountIdentity,
) -> std::result::Result<Vec<PeerInfo>, engine::Gap> {
    let Some((users, chats)) = value.peers() else {
        return Ok(vec![]);
    };
    let capacity = (UpdateScope::Account, UpdateGapReason::BacklogLimit);
    // Only fixed-size routing records are copied. The engine checks the full
    // envelope byte budget independently, even when it must refuse that data.
    if users.len().saturating_add(chats.len()) > 1024 {
        return Err(capacity);
    }
    common::checked_entities_ref(users, chats, expected)
        .map_err(|_| (UpdateScope::Account, UpdateGapReason::Malformed))
}
type Entities<'a> = Option<(&'a [tl::enums::User], &'a [tl::enums::Chat])>;
pub(super) trait Metadata {
    fn peers(&self) -> Entities<'_>;
}
impl Metadata for tl::enums::updates::Difference {
    fn peers(&self) -> Entities<'_> {
        match self {
            Self::Difference(value) => Some((&value.users, &value.chats)),
            Self::Slice(value) => Some((&value.users, &value.chats)),
            _ => None,
        }
    }
}
impl Metadata for tl::enums::updates::ChannelDifference {
    fn peers(&self) -> Entities<'_> {
        match self {
            Self::Difference(value) => Some((&value.users, &value.chats)),
            Self::TooLong(value) => Some((&value.users, &value.chats)),
            _ => None,
        }
    }
}
impl Metadata for UpdatesLike {
    fn peers(&self) -> Entities<'_> {
        match self {
            Self::Updates(value) => container(value),
            Self::InvitedUsers(value) => container(&value.updates),
            Self::ChatInviteJoinResult(value) => container(&value.updates),
            _ => None,
        }
    }
}
fn container(value: &tl::enums::Updates) -> Entities<'_> {
    match value {
        tl::enums::Updates::Updates(value) => Some((&value.users, &value.chats)),
        tl::enums::Updates::Combined(value) => Some((&value.users, &value.chats)),
        _ => None,
    }
}
