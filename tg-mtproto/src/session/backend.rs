//! The key-bearing upstream trait stays behind the public Session boundary.
use super::{
    SessionError, SessionErrorKind,
    data::{peer_id, validate_updates},
    types::{Result, error, storage},
    worker::Database,
};
use grammers_session::{
    BoxFuture, Session,
    types::{ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use std::collections::HashMap;
use zeroize::Zeroize;

pub(super) struct BackendSession {
    pub database: Database,
}
impl BackendSession {
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}
struct PendingDc(DcOption);
impl Drop for PendingDc {
    fn drop(&mut self) {
        if let Some(key) = &mut self.0.auth_key {
            key.zeroize();
        }
    }
}
impl Session for BackendSession {
    type Error = SessionError;
    fn home_dc_id(&self) -> Result<i32> {
        self.database.shared.read(|view| view.snapshot.home_dc)
    }
    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.database
                .call(move |native| {
                    native.mutate(|snapshot| {
                        snapshot.home_dc = dc_id;
                        Ok(())
                    })
                })
                .await
        })
    }
    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>> {
        self.database.shared.read(|view| {
            view.snapshot
                .datacenters
                .iter()
                .find(|dc| dc.id == dc_id)
                .cloned()
        })
    }
    fn set_dc_option(&self, option: &DcOption) -> BoxFuture<'_, Result<()>> {
        let option = PendingDc(option.clone());
        Box::pin(async move {
            self.database
                .call(move |native| {
                    native.mutate(|snapshot| {
                        if let Some(existing) = snapshot
                            .datacenters
                            .iter_mut()
                            .find(|dc| dc.id == option.0.id)
                        {
                            if let Some(key) = &mut existing.auth_key {
                                key.zeroize();
                            }
                            *existing = option.0.clone();
                        } else {
                            snapshot.datacenters.push(option.0.clone());
                        }
                        Ok(())
                    })
                })
                .await
        })
    }
    fn peer(&self, id: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>>> {
        Box::pin(async move {
            self.database.shared.read(|view| {
                view.snapshot
                    .peers
                    .iter()
                    .find(|peer| {
                        if id == PeerId::self_user() {
                            matches!(
                                peer,
                                PeerInfo::User {
                                    is_self: Some(true),
                                    ..
                                }
                            )
                        } else {
                            peer.id() == id
                        }
                    })
                    .cloned()
            })
        })
    }
    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<()>> {
        self.cache_peers(vec![peer])
    }
    fn cache_peers(&self, peers: Vec<PeerInfo>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if peers.len() > self.database.options.max_peers
                || peers.capacity() > self.database.options.max_peers
            {
                return Err(error(SessionErrorKind::Capacity));
            }
            for peer in &peers {
                peer_id(peer)?;
            }
            self.database
                .call(move |native| {
                    native.mutate(|snapshot| {
                        let mut positions: HashMap<_, _> = snapshot
                            .peers
                            .iter()
                            .enumerate()
                            .map(|(index, peer)| (peer.id(), index))
                            .collect();
                        for peer in peers {
                            let id = peer_id(&peer)?;
                            if let Some(index) = positions.get(&id) {
                                snapshot.peers[*index].extend_info(&peer);
                            } else {
                                positions.insert(id, snapshot.peers.len());
                                snapshot.peers.push(peer);
                            }
                        }
                        Ok(())
                    })
                })
                .await
        })
    }
    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState>> {
        Box::pin(async move { self.database.shared.read(|view| view.provisional.clone()) })
    }
    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if let UpdateState::All(state) = &update {
                if state.channels.capacity() > self.database.options.max_channels {
                    return Err(error(SessionErrorKind::Capacity));
                }
                validate_updates(state, &self.database.options)?;
            }
            self.database
                .call(move |native| {
                    let mut view = native.shared.view.write().map_err(storage)?;
                    let view = view
                        .as_mut()
                        .ok_or_else(|| error(SessionErrorKind::Closed))?;
                    let mut next = view.provisional.clone();
                    native.live()?;
                    match update {
                        UpdateState::All(state) => next = state,
                        UpdateState::Primary { pts, date, seq } => {
                            next.pts = pts;
                            next.date = date;
                            next.seq = seq;
                        }
                        UpdateState::Secondary { qts } => next.qts = qts,
                        UpdateState::Channel { id, pts } => {
                            if let Some(channel) =
                                next.channels.iter_mut().find(|channel| channel.id == id)
                            {
                                channel.pts = pts;
                            } else {
                                next.channels.push(ChannelState { id, pts });
                            }
                        }
                    }
                    validate_updates(&next, &native.options)?;
                    native.live()?;
                    view.provisional = next;
                    Ok(())
                })
                .await
        })
    }
}
