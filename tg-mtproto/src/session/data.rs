use super::{
    SessionErrorKind, SessionOptions,
    types::{Result, error},
};
use grammers_session::{
    SessionData,
    types::{DcOption, PeerId, PeerInfo, UpdatesState},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use zeroize::Zeroize;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub version: u8,
    pub quarantined: bool,
    pub home_dc: i32,
    pub datacenters: Vec<DcOption>,
    pub peers: Vec<PeerInfo>,
    pub acknowledged: Option<UpdatesState>,
    pub account: Option<super::AccountIdentity>,
}
impl Default for Snapshot {
    fn default() -> Self {
        let data = SessionData::default();
        Self {
            version: 2,
            quarantined: false,
            home_dc: data.home_dc,
            datacenters: data.dc_options.into_values().collect(),
            peers: vec![],
            acknowledged: None,
            account: None,
        }
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        for option in &mut self.datacenters {
            if let Some(key) = &mut option.auth_key {
                key.zeroize();
            }
        }
    }
}
impl Snapshot {
    pub fn validate(&self, options: &SessionOptions) -> Result<()> {
        if self.version != 2 {
            return Err(error(SessionErrorKind::UnsupportedSchema));
        }
        if self.datacenters.len() > options.max_datacenters || self.peers.len() > options.max_peers
        {
            return Err(error(SessionErrorKind::Capacity));
        }
        let mut dcs = HashSet::new();
        for dc in &self.datacenters {
            if dc.id == 0 || !dcs.insert(dc.id) {
                return Err(error(SessionErrorKind::Authentication));
            }
        }
        if !dcs.contains(&self.home_dc) {
            return Err(error(SessionErrorKind::Authentication));
        }
        let mut peers = HashSet::new();
        let mut selves = 0;
        for peer in &self.peers {
            let id = match peer {
                PeerInfo::User { id, is_self, .. } => {
                    selves += usize::from(*is_self == Some(true));
                    PeerId::user(*id)
                }
                PeerInfo::Chat { id } => PeerId::chat(*id),
                PeerInfo::Channel { id, .. } => PeerId::channel(*id),
            }
            .ok_or_else(|| error(SessionErrorKind::Authentication))?;
            if !peers.insert(id) || selves > 1 {
                return Err(error(SessionErrorKind::Authentication));
            }
        }
        if let Some(identity) = self.account {
            super::AccountId::new(identity.id.get())?;
            if !self.peers.iter().any(|peer|matches!(peer,PeerInfo::User{id,bot:Some(bot),is_self:Some(true),..} if *id==identity.id.get() && *bot==(identity.kind==super::AccountKind::Bot))) {return Err(error(SessionErrorKind::Authentication));}
        }
        if let Some(state) = &self.acknowledged {
            validate_updates(state, options)?;
        }
        Ok(())
    }
}
pub(super) fn peer_id(peer: &PeerInfo) -> Result<PeerId> {
    match peer {
        PeerInfo::User { id, .. } => PeerId::user(*id),
        PeerInfo::Chat { id } => PeerId::chat(*id),
        PeerInfo::Channel { id, .. } => PeerId::channel(*id),
    }
    .ok_or_else(|| error(SessionErrorKind::InvalidInput))
}
pub(super) fn validate_updates(state: &UpdatesState, options: &SessionOptions) -> Result<()> {
    if state.channels.len() > options.max_channels {
        return Err(error(SessionErrorKind::Capacity));
    }
    if state.pts < 0 || state.qts < 0 || state.date < 0 || state.seq < 0 {
        return Err(error(SessionErrorKind::Authentication));
    }
    let mut channels = HashSet::new();
    for channel in &state.channels {
        if channel.pts < 0 || PeerId::channel(channel.id).is_none() || !channels.insert(channel.id)
        {
            return Err(error(SessionErrorKind::Authentication));
        }
    }
    Ok(())
}
