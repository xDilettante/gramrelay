mod backend;
mod codec;
mod data;
#[cfg(test)]
mod tests;
mod types;
mod worker;
pub(crate) use backend::BackendSession;
use data::Snapshot;
use grammers_session::types::UpdatesState;
use std::sync::atomic::Ordering;
use std::{
    fmt,
    path::Path,
    sync::{Arc, RwLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
pub use types::{
    AccountId, AccountIdentity, AccountKind, SessionBinding, SessionError, SessionErrorKind,
    SessionKey, SessionOptions,
};
use types::{Result, error, storage};

#[derive(Clone)]
pub struct Session {
    inner: Arc<backend::BackendSession>,
}
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session([REDACTED])")
    }
}
impl Session {
    /// Opens a dedicated protected database without connecting to Telegram.
    pub async fn open(
        path: impl AsRef<Path>,
        binding: SessionBinding,
        key: SessionKey,
        options: SessionOptions,
    ) -> Result<Self> {
        options.validate()?;
        if !path.as_ref().is_absolute() {
            return Err(error(SessionErrorKind::InvalidInput));
        }
        Ok(Self {
            inner: Arc::new(backend::BackendSession::new(
                worker::Database::open(path.as_ref().to_owned(), binding, key, options).await?,
            )),
        })
    }
    /// Stored identity is metadata; the Client must verify it with Telegram.
    pub fn remembered_account(&self) -> Result<Option<AccountIdentity>> {
        self.inner
            .database
            .shared
            .read(|view| view.snapshot.account)
    }
    pub fn binding(&self) -> &SessionBinding {
        &self.inner.database.binding
    }
    /// Local quarantine policy only; false is not proof of remote authorization.
    pub fn is_quarantined(&self) -> Result<bool> {
        self.inner
            .database
            .shared
            .read(|view| view.snapshot.quarantined)
    }
    pub(crate) fn has_datacenter(&self, dc: i32) -> Result<bool> {
        self.inner.database.shared.read(|view| {
            view.snapshot
                .datacenters
                .iter()
                .any(|option| option.id == dc)
        })
    }
    pub(crate) async fn quarantine_authorization(&self) -> Result<()> {
        self.inner
            .database
            .call(|native| {
                native.mutate(|snapshot| {
                    snapshot.quarantined = true;
                    Ok(())
                })
            })
            .await
    }
    /// Explicitly discard quarantined local keys/state without a network request.
    /// This does not revoke an unresolved remote authorization. An attached
    /// Client must be closed and its Session reopened before local recovery.
    pub async fn forget_quarantined_authorization(&self) -> Result<()> {
        let claim = self.claim_client()?;
        self.inner
            .database
            .call(move |native| {
                let result = native.clear_authorization();
                drop(claim);
                result
            })
            .await
    }
    pub(crate) async fn remember_verified(
        &self,
        identity: AccountIdentity,
        peer: grammers_session::types::PeerInfo,
    ) -> Result<()> {
        use grammers_session::types::PeerInfo;
        if !matches!(&peer, PeerInfo::User {id, bot:Some(bot), is_self:Some(true), ..} if *id==identity.id.get() && *bot==(identity.kind==AccountKind::Bot))
        {
            return Err(error(SessionErrorKind::InvalidInput));
        }
        self.inner.database.call(move |native| {
            native.mutate(|snapshot| {
                if snapshot.quarantined { return Err(error(SessionErrorKind::Quarantined)); }
                if snapshot.account.is_some_and(|stored| stored != identity)
                    || snapshot.peers.iter().any(|peer| matches!(peer, PeerInfo::User {id, is_self:Some(true), ..} if *id!=identity.id.get()))
                {
                    return Err(error(SessionErrorKind::Authentication));
                }
                if let Some(stored) = snapshot.peers.iter_mut().find(|peer| matches!(peer, PeerInfo::User {id,..} if *id==identity.id.get())) {
                    *stored = peer;
                } else {
                    snapshot.peers.push(peer);
                }
                snapshot.account = Some(identity);
                Ok(())
            })
        }).await
    }
    /// Stop admission on all clones, drain native jobs and release ownership.
    /// Returns `Owned` while attached to a Client; use `Client::close` instead.
    pub async fn close(&self) -> Result<()> {
        match self
            .inner
            .owner
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(2) => {}
            Err(_) => return Err(error(SessionErrorKind::Owned)),
        }
        self.inner.database.close().await
    }
    pub(crate) fn claim_client(&self) -> Result<ClientClaim> {
        self.inner.database.shared.read(|_| ())?;
        self.inner
            .owner
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| error(SessionErrorKind::Owned))?;
        if let Err(failure) = self.inner.database.shared.read(|_| ()) {
            let _ = self
                .inner
                .owner
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
            return Err(failure);
        }
        Ok(ClientClaim {
            session: self.clone(),
            started: false,
        })
    }
    pub(crate) fn live(&self) -> Result<()> {
        self.inner.database.shared.read(|_| ())
    }
    #[cfg(test)]
    pub(crate) fn test_fence(&self) {
        self.inner
            .database
            .shared
            .health
            .send_replace(Health::Fenced);
    }
}

pub(crate) struct ClientClaim {
    session: Session,
    started: bool,
}
impl ClientClaim {
    pub fn session(&self) -> Session {
        self.session.clone()
    }
    pub fn api_id(&self) -> i32 {
        self.session.binding().api_id()
    }
    pub fn backend(&self) -> Arc<BackendSession> {
        self.session.inner.clone()
    }
    pub fn start(&mut self) {
        self.started = true;
    }
    pub fn live(&self) -> Result<()> {
        self.session.live()
    }
    pub fn deadline(&self) -> Result<Instant> {
        self.session
            .inner
            .database
            .shared
            .read(|view| view.deadline)
    }
    pub fn health(&self) -> watch::Receiver<Health> {
        self.session.inner.database.shared.health.subscribe()
    }
    pub async fn finish(&self) -> Result<()> {
        self.session.inner.owner.store(2, Ordering::Release);
        self.session.inner.database.close().await
    }
    pub async fn finish_logged_out(&self) -> Result<()> {
        self.session.inner.database.request_credential_clear();
        self.finish().await
    }
}
impl Drop for ClientClaim {
    fn drop(&mut self) {
        if !self.started {
            let _ = self.session.inner.owner.compare_exchange(
                1,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Health {
    Opening,
    Ready,
    Closing,
    Closed,
    Fenced,
}
struct View {
    snapshot: Snapshot,
    provisional: UpdatesState,
    lease_until_ms: i64,
    deadline: Instant,
}
struct Shared {
    view: RwLock<Option<View>>,
    health: watch::Sender<Health>,
}
impl Shared {
    fn health(&self) -> Health {
        *self.health.borrow()
    }
    fn new() -> Self {
        Self {
            view: RwLock::new(None),
            health: watch::channel(Health::Opening).0,
        }
    }
    fn read<T>(&self, read: impl FnOnce(&View) -> T) -> Result<T> {
        match self.health() {
            Health::Ready => {}
            Health::Fenced => return Err(error(SessionErrorKind::Fenced)),
            _ => return Err(error(SessionErrorKind::Closed)),
        }
        let view = self.view.read().map_err(storage)?;
        let view = view
            .as_ref()
            .ok_or_else(|| error(SessionErrorKind::Closed))?;
        if Instant::now() >= view.deadline || now()? >= view.lease_until_ms {
            self.health.send_replace(Health::Fenced);
            return Err(error(SessionErrorKind::Fenced));
        }
        let value = read(view);
        if Instant::now() >= view.deadline
            || now()? >= view.lease_until_ms
            || self.health() == Health::Fenced
        {
            self.health.send_replace(Health::Fenced);
            return Err(error(SessionErrorKind::Fenced));
        }
        if self.health() != Health::Ready {
            return Err(error(SessionErrorKind::Closed));
        }
        Ok(value)
    }
}
fn now() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(storage)?
            .as_millis(),
    )
    .map_err(storage)
}
