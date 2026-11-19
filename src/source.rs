use crate::{Bot, Error, ErrorKind, Result, User};
use std::{
    collections::HashSet,
    fmt,
    sync::{Mutex, OnceLock},
};

static OWNERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Process-local ownership shared by managed polling and webhook integrations.
/// Raw API calls and collectors in other processes require separate coordination.
pub struct UpdateSourceGuard {
    key: String,
}
impl fmt::Debug for UpdateSourceGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateSourceGuard").finish_non_exhaustive()
    }
}
impl Drop for UpdateSourceGuard {
    fn drop(&mut self) {
        if let Some(owners) = OWNERS.get()
            && let Ok(mut owners) = owners.lock()
        {
            owners.remove(&self.key);
        }
    }
}
impl Bot {
    /// Claim a process-local source slot, keyed by numeric bot ID rather than token secret.
    /// Holding this guard performs no network request.
    pub fn claim_update_source(&self) -> Result<UpdateSourceGuard> {
        let mut owners = OWNERS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| Error::local(ErrorKind::Configuration, "polling_registry"))?;
        let key = self.identity_key();
        if !owners.insert(key.to_owned()) {
            return Err(Error::local(ErrorKind::Capacity, "polling_owner"));
        }
        Ok(UpdateSourceGuard {
            key: key.to_owned(),
        })
    }

    /// Fetch getMe and verify that its bot identity matches this client's token ID.
    pub async fn verified_identity(&self) -> Result<User> {
        let identity = self.get_me().await?;
        if !identity.is_bot
            || self.identity_key().parse::<i64>().ok() != Some(i64::from(identity.id))
        {
            return Err(Error::local(ErrorKind::Configuration, "bot_identity"));
        }
        Ok(identity)
    }
}
