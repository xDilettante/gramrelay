use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionErrorKind {
    InvalidInput,
    Storage,
    Authentication,
    UnsupportedSchema,
    Capacity,
    Owned,
    Closed,
    Fenced,
    Quarantined,
}
#[derive(Clone, Copy, Debug)]
pub struct SessionError {
    kind: SessionErrorKind,
}
impl SessionError {
    pub fn kind(&self) -> SessionErrorKind {
        self.kind
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            SessionErrorKind::InvalidInput => "mtproto_session_invalid_input",
            SessionErrorKind::Storage => "mtproto_session_storage",
            SessionErrorKind::Authentication => "mtproto_session_authentication",
            SessionErrorKind::UnsupportedSchema => "mtproto_session_schema",
            SessionErrorKind::Capacity => "mtproto_session_capacity",
            SessionErrorKind::Owned => "mtproto_session_owned",
            SessionErrorKind::Closed => "mtproto_session_closed",
            SessionErrorKind::Fenced => "mtproto_session_fenced",
            SessionErrorKind::Quarantined => "mtproto_session_quarantined",
        }
    }
}
impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for SessionError {}
pub(super) type Result<T> = std::result::Result<T, SessionError>;
pub(super) fn error(kind: SessionErrorKind) -> SessionError {
    SessionError { kind }
}
pub(super) fn storage(_: impl Sized) -> SessionError {
    error(SessionErrorKind::Storage)
}

/// A host-provisioned cryptographically random key. Never serialize or log it.
pub struct SessionKey(pub(super) Zeroizing<[u8; 32]>);
impl SessionKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}
impl fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionKey([REDACTED])")
    }
}

/// Authenticated host context; changing either field cannot reopen the same blob.
#[derive(Clone, Serialize)]
pub struct SessionBinding {
    pub(super) profile: String,
    pub(super) api_id: i32,
}
impl SessionBinding {
    pub fn new(profile: impl Into<String>, api_id: i32) -> Result<Self> {
        let profile = profile.into();
        if profile.is_empty()
            || profile.len() > 64
            || !profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            || api_id <= 0
        {
            return Err(error(SessionErrorKind::InvalidInput));
        }
        Ok(Self { profile, api_id })
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn api_id(&self) -> i32 {
        self.api_id
    }
}
impl fmt::Debug for SessionBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionBinding([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AccountId(i64);
impl AccountId {
    pub fn new(value: i64) -> Result<Self> {
        grammers_session::types::PeerId::user(value)
            .ok_or_else(|| error(SessionErrorKind::InvalidInput))?;
        Ok(Self(value))
    }
    pub fn get(self) -> i64 {
        self.0
    }
}
impl<'de> Deserialize<'de> for AccountId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::new(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccountKind {
    User,
    Bot,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountIdentity {
    pub id: AccountId,
    pub kind: AccountKind,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    #[cfg(test)]
    pub(super) after_open_commit: Option<Duration>,
    pub(super) max_snapshot_bytes: usize,
    pub(super) max_peers: usize,
    pub(super) max_channels: usize,
    pub(super) max_datacenters: usize,
    pub(super) max_pending: usize,
    pub(super) lease: Duration,
    pub(super) busy_timeout: Duration,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            after_open_commit: None,
            max_snapshot_bytes: 8 * 1024 * 1024,
            max_peers: 10000,
            max_channels: 10000,
            max_datacenters: 32,
            max_pending: 16,
            lease: Duration::from_secs(60),
            busy_timeout: Duration::from_secs(2),
        }
    }
}
impl SessionOptions {
    pub fn max_snapshot_bytes(mut self, bytes: usize) -> Self {
        self.max_snapshot_bytes = bytes;
        self
    }
    pub fn max_peers(mut self, count: usize) -> Self {
        self.max_peers = count;
        self
    }
    pub fn max_channels(mut self, count: usize) -> Self {
        self.max_channels = count;
        self
    }
    pub fn max_pending(mut self, count: usize) -> Self {
        self.max_pending = count;
        self
    }
    pub fn lease(mut self, duration: Duration) -> Self {
        self.lease = duration;
        self
    }
    pub fn busy_timeout(mut self, duration: Duration) -> Self {
        self.busy_timeout = duration;
        self
    }
    pub fn lease_duration(&self) -> Duration {
        self.lease
    }
    pub(super) fn validate(&self) -> Result<()> {
        if !(1024..=64 * 1024 * 1024).contains(&self.max_snapshot_bytes)
            || self.max_peers == 0
            || self.max_peers > 1_000_000
            || self.max_channels == 0
            || self.max_channels > 1_000_000
            || self.max_datacenters == 0
            || self.max_datacenters > 128
            || self.max_pending == 0
            || self.max_pending > 1024
            || self.lease < Duration::from_millis(100)
            || self.lease > Duration::from_secs(86400)
            || self.busy_timeout.is_zero()
            || self.busy_timeout > Duration::from_secs(5)
        {
            Err(error(SessionErrorKind::InvalidInput))
        } else {
            Ok(())
        }
    }
}
