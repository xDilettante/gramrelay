mod backend;
mod codec;
mod data;
#[cfg(test)]
mod tests;
mod types;
mod worker;
use data::Snapshot;
use grammers_session::types::UpdatesState;
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
    /// Stop admission on all clones, drain native jobs and release ownership.
    pub async fn close(&self) -> Result<()> {
        self.inner.database.close().await
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Health {
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
