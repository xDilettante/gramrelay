//! Immutable host-provided account metadata. A binding is never an execution grant.
//!
//! Register expected metadata before connecting an MTProto executor (M3).
//! Live identity verification and current grants remain separate requirements.
//!
//! ```no_run
//! use tg_agent::{journal::{Journal, JournalError}, profiles::ProfileIdentity};
//!
//! async fn bind_expected_user(journal: &Journal, expected_id: i64) -> Result<(), JournalError> {
//!     let identity = ProfileIdentity::mtproto_user(expected_id)?;
//!     journal.bind_profile("personal", identity).await
//! }
//! ```
use crate::journal::{
    Journal, JournalError, JournalErrorKind, JournalOptions, Result, storage, types::valid_id,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProfileKind {
    BotApi,
    MtprotoBot,
    MtprotoUser,
}
impl ProfileKind {
    fn code(self) -> &'static str {
        match self {
            Self::BotApi => "bot_api",
            Self::MtprotoBot => "mtproto_bot",
            Self::MtprotoUser => "mtproto_user",
        }
    }
}

/// Exact expected identity, containing no credentials and no live verification.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProfileIdentity {
    kind: ProfileKind,
    id: i64,
}
impl ProfileIdentity {
    /// Nonnegative to retain compatibility with synthetic Bot API ID-zero fixtures.
    pub fn bot_api(id: i64) -> Result<Self> {
        Self::new(ProfileKind::BotApi, id)
    }
    pub fn mtproto_bot(id: i64) -> Result<Self> {
        Self::new(ProfileKind::MtprotoBot, id)
    }
    pub fn mtproto_user(id: i64) -> Result<Self> {
        Self::new(ProfileKind::MtprotoUser, id)
    }
    fn new(kind: ProfileKind, id: i64) -> Result<Self> {
        if id < 0 || (id == 0 && kind != ProfileKind::BotApi) {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        Ok(Self { kind, id })
    }
    pub fn kind(self) -> ProfileKind {
        self.kind
    }
    /// Intentional metadata access; Debug omits the ID.
    pub fn account_id(self) -> i64 {
        self.id
    }
}
impl fmt::Debug for ProfileIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileIdentity")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Journal {
    /// Durably bind one host profile. Does not verify login or grant Telegram access.
    pub async fn bind_profile(
        &self,
        profile: impl Into<String>,
        identity: ProfileIdentity,
    ) -> Result<()> {
        let profile = profile.into();
        valid_id(&profile)?;
        self.run(move |connection, limits, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            bind(&transaction, limits, &profile, identity)?;
            transaction.commit().map_err(|_| storage())
        })
        .await
    }
    pub async fn profile_identity(&self, profile: &str) -> Result<Option<ProfileIdentity>> {
        valid_id(profile)?;
        let profile = profile.to_owned();
        self.run(move |connection, _, _| load(connection, &profile))
            .await
    }
}

pub(crate) fn load(connection: &Connection, profile: &str) -> Result<Option<ProfileIdentity>> {
    let value: Option<(String, i64)> = connection
        .query_row(
            "SELECT kind,account_id FROM account_profiles WHERE profile=?1",
            [profile],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| storage())?;
    value
        .map(|(kind, id)| {
            let kind = match kind.as_str() {
                "bot_api" => ProfileKind::BotApi,
                "mtproto_bot" => ProfileKind::MtprotoBot,
                "mtproto_user" => ProfileKind::MtprotoUser,
                _ => return Err(storage()),
            };
            ProfileIdentity::new(kind, id).map_err(|_| storage())
        })
        .transpose()
}

/// Called only under the same write transaction as a new binding's consumer.
pub(crate) fn bind(
    connection: &Connection,
    limits: &JournalOptions,
    profile: &str,
    identity: ProfileIdentity,
) -> Result<()> {
    if let Some(current) = load(connection, profile)? {
        return if current == identity {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    // Generic old journals cannot identify an account from an operation or quota.
    // Retained rows must not be reinterpreted as a newly chosen account.
    let occupied: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE profile=?1) OR EXISTS(SELECT 1 FROM executor_budgets WHERE profile=?1) OR EXISTS(SELECT 1 FROM executor_files WHERE profile=?1) OR EXISTS(SELECT 1 FROM update_feeds WHERE profile=?1)",
        [profile], |row| row.get(0),
    ).map_err(|_| storage())?;
    let alias: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM account_profiles WHERE kind=?1 AND account_id=?2)",
            params![identity.kind.code(), identity.id],
            |row| row.get(0),
        )
        .map_err(|_| storage())?;
    if occupied || alias {
        return Err(conflict());
    }
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM account_profiles", [], |row| {
            row.get(0)
        })
        .map_err(|_| storage())?;
    if count >= limits.max_profiles as i64 {
        return Err(JournalError::new(JournalErrorKind::Capacity));
    }
    connection
        .execute(
            "INSERT INTO account_profiles(profile,kind,account_id) VALUES(?1,?2,?3)",
            params![profile, identity.kind.code(), identity.id],
        )
        .map_err(|_| storage())?;
    Ok(())
}
fn conflict() -> JournalError {
    JournalError::new(JournalErrorKind::Conflict)
}
