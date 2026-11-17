//! Trusted operator bootstrap. Config/paths/environment are never tool arguments.
use crate::{
    executor::{
        Action, Budget, BudgetLimits, CallbackClockPolicy, ChatId, ChatScope, Executor,
        ExecutorErrorKind, ExecutorOptions, FileLimits, Grant, PolicyStore, Principal, Request,
        deserialize_object, validate_cursor, validate_identifier,
    },
    journal::{Journal, JournalOptions},
    tools::{ToolError, ToolSession},
    updates::{UpdateCursor, UpdateFeed, UpdateOptions},
};
use serde::{Deserialize, Deserializer};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt;
#[cfg(feature = "mtproto")]
mod mtproto;

/// Owns the journal lifetime of a host-configured session. Drain callers before close.
pub struct Host {
    session: ToolSession,
    journal: Journal,
    #[cfg(feature = "mtproto")]
    mtproto: Option<mtproto::Resources>,
}
impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Host([REDACTED])")
    }
}
impl Host {
    /// Loads at most 1 MiB from an absolute regular config file. No Telegram RPC.
    pub async fn load(path: impl AsRef<Path>) -> Result<Self, ToolError> {
        #[cfg(feature = "mtproto")]
        return Self::load_inner(path.as_ref(), Box::new(tg_mtproto::Client::new)).await;
        #[cfg(not(feature = "mtproto"))]
        Self::load_inner(path.as_ref()).await
    }
    /// Development-only lazy M2 factory. No config field selects this factory.
    #[cfg(feature = "mtproto-test-kit")]
    pub async fn load_with_mtproto_factory<F>(
        path: impl AsRef<Path>,
        factory: F,
    ) -> Result<Self, ToolError>
    where
        F: FnMut(
                tg_mtproto::Session,
                tg_mtproto::ClientOptions,
            ) -> Result<tg_mtproto::Client, tg_mtproto::ClientError>
            + Send
            + 'static,
    {
        Self::load_inner(path.as_ref(), Box::new(factory)).await
    }
    async fn load_inner(
        path: &Path,
        #[cfg(feature = "mtproto")] factory: mtproto::Factory,
    ) -> Result<Self, ToolError> {
        let bytes = read_regular(path, 1024 * 1024).await?;
        let config: Strict<Config> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let prepared = config.0.prepare()?;
        #[cfg(feature = "mtproto")]
        let mut mtproto = match prepared.mtproto {
            Some(prepared_mt) => Some(prepared_mt.open(&prepared.journal_path, factory).await?),
            None => None,
        };
        let journal = match Journal::open(&prepared.journal_path, JournalOptions::default()).await {
            Ok(journal) => journal,
            Err(_) => {
                #[cfg(feature = "mtproto")]
                if let Some(mt) = &mtproto {
                    let _ = mt.resources.close().await;
                }
                return Err(invalid());
            }
        };
        let session = async {
            let mut feeds = BTreeMap::new();
            for profile in prepared.profiles {
                let feed = UpdateFeed::open(
                    journal.clone(),
                    &profile.name,
                    profile.bot_id,
                    profile.options,
                )
                .await
                .map_err(|_| invalid())?;
                prepared
                    .executor
                    .register_profile(feed.clone(), profile.bot)
                    .map_err(|_| invalid())?;
                feeds.insert(profile.name, feed);
            }
            for Strict(observation) in prepared.observations {
                let feed = feeds.get(&observation.profile).ok_or_else(invalid)?;
                let event = feed
                    .event_at(&observation.cursor)
                    .await
                    .map_err(|_| invalid())?
                    .ok_or_else(invalid)?;
                prepared
                    .executor
                    .observe_callback(&prepared.principal, &observation.profile, &event)
                    .map_err(|_| invalid())?;
            }
            for Strict(restore) in prepared.restorations {
                let bytes =
                    read_regular(&restore.path, FileLimits::default().max_file_bytes).await?;
                prepared
                    .executor
                    .restore_file(
                        &prepared.principal,
                        &restore.profile,
                        &restore.file_handle,
                        bytes,
                    )
                    .await
                    .map_err(|_| invalid())?;
            }
            let session = ToolSession::new(prepared.executor, prepared.principal.clone());
            for Strict(approval) in prepared.approvals {
                session
                    .approve(&approval.request, approval.expires_at_ms)
                    .await
                    .map_err(|_| invalid())?;
            }
            #[cfg(feature = "mtproto")]
            let session = match &mut mtproto {
                Some(mt) => mt.attach(&journal, &prepared.principal, session).await?,
                None => session,
            };
            Ok::<_, ToolError>(session)
        }
        .await;
        match session {
            Ok(session) => Ok(Self {
                session,
                journal,
                #[cfg(feature = "mtproto")]
                mtproto: mtproto.map(|mt| mt.resources),
            }),
            Err(error) => {
                #[cfg(feature = "mtproto")]
                if let Some(mt) = &mtproto {
                    let _ = mt.resources.close().await;
                }
                let _ = journal.close().await;
                Err(error)
            }
        }
    }
    pub fn session(&self) -> &ToolSession {
        &self.session
    }
    /// Drain adapter calls first. A cancelled close wait can be resumed.
    pub async fn close(&self) -> Result<(), ToolError> {
        #[cfg(feature = "mtproto")]
        let mt_closed = match &self.mtproto {
            Some(mt) => mt.close().await,
            None => Ok(()),
        };
        let journal_closed = self
            .journal
            .close()
            .await
            .map_err(|_| ToolError::new("agent_storage", "Host journal close failed."));
        #[cfg(feature = "mtproto")]
        return mt_closed.and(journal_closed);
        #[cfg(not(feature = "mtproto"))]
        journal_closed
    }
}
async fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, ToolError> {
    absolute(path)?;
    let file = tokio::fs::File::open(path).await.map_err(|_| invalid())?;
    let metadata = file.metadata().await.map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(bytes)
}
fn absolute(path: &Path) -> Result<(), ToolError> {
    if !path.is_absolute() || path.as_os_str().to_string_lossy().contains('\0') {
        return Err(invalid());
    }
    Ok(())
}
fn id(value: &str) -> Result<(), ToolError> {
    validate_identifier(value).map_err(|_| invalid())
}
fn unique<T: Ord>(values: &[T]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
fn invalid() -> ToolError {
    ToolError::new(
        "agent_config",
        "Invalid or unavailable trusted host configuration.",
    )
}

// Reuse the executor's map-only visitor so config structs cannot accept Serde's
// positional-array aliases. Derived visitors still reject duplicate/unknown keys.
#[derive(Default)]
struct Strict<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Strict<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_object(deserializer).map(Self)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    principal: String,
    journal_path: PathBuf,
    profiles: Vec<Strict<Profile>>,
    #[serde(default)]
    approvals: Vec<Strict<Confirmation>>,
    #[serde(default)]
    file_restorations: Vec<Strict<Restoration>>,
    #[serde(default)]
    callback_clock_policy: ClockPolicy,
    #[serde(default)]
    callback_observations: Vec<Strict<Observation>>,
    #[cfg(feature = "mtproto")]
    #[serde(default, deserialize_with = "present_mtproto")]
    mtproto: Option<Strict<mtproto::Config>>,
}
#[cfg(feature = "mtproto")]
fn present_mtproto<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<Strict<mtproto::Config>>, D::Error> {
    Strict::deserialize(d).map(Some)
}
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClockPolicy {
    #[default]
    JournalMonotonic,
    TrustedWallClock,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    bot_id: String,
    token_env: String,
    grant: Strict<GrantConfig>,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    allow_http_loopback: bool,
    #[serde(default)]
    update_options: Strict<FeedConfig>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedConfig {
    #[serde(default)]
    retention_seconds: Option<u64>,
    #[serde(default)]
    max_event_bytes: Option<usize>,
    #[serde(default)]
    max_batch_bytes: Option<usize>,
}
impl FeedConfig {
    fn build(self) -> Result<UpdateOptions, ToolError> {
        let mut options = UpdateOptions::default();
        if let Some(seconds) = self.retention_seconds {
            options = options.retention(Duration::from_secs(seconds));
        }
        if let Some(bytes) = self.max_event_bytes {
            options = options.max_event_bytes(bytes);
        }
        if let Some(bytes) = self.max_batch_bytes {
            options = options.max_batch_bytes(bytes);
        }
        options
            .validate(&JournalOptions::default())
            .map_err(|_| invalid())?;
        Ok(options)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantConfig {
    #[serde(default)]
    actions: Vec<Action>,
    #[serde(default)]
    chats: Vec<ChatId>,
    #[serde(default)]
    all_chats: bool,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    unscoped_updates: bool,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    #[serde(default)]
    require_approval: Vec<Action>,
    #[serde(default)]
    costs: Vec<Strict<Cost>>,
    budget: Strict<BudgetConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetConfig {
    id: String,
    #[serde(default = "window")]
    window_seconds: u64,
    #[serde(default)]
    limits: Strict<Caps>,
}
fn window() -> u64 {
    60
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Caps {
    invocations: u64,
    write_attempts: u64,
    upload_bytes: u64,
    cost_units: u64,
}
impl Default for Caps {
    fn default() -> Self {
        let limits = BudgetLimits::default();
        Self {
            invocations: limits.invocations,
            write_attempts: limits.write_attempts,
            upload_bytes: limits.upload_bytes,
            cost_units: limits.cost_units,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cost {
    action: Action,
    units: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    request: Request,
    expires_at_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Restoration {
    profile: String,
    file_handle: String,
    path: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    profile: String,
    cursor: UpdateCursor,
}
struct PreparedProfile {
    name: String,
    bot_id: i64,
    bot: tg::Bot,
    options: UpdateOptions,
}
struct Prepared {
    executor: Executor,
    principal: Principal,
    journal_path: PathBuf,
    profiles: Vec<PreparedProfile>,
    restorations: Vec<Strict<Restoration>>,
    approvals: Vec<Strict<Confirmation>>,
    observations: Vec<Strict<Observation>>,
    #[cfg(feature = "mtproto")]
    mtproto: Option<mtproto::Prepared>,
}
impl Config {
    fn prepare(self) -> Result<Prepared, ToolError> {
        let counts: (usize, usize, usize, usize) = (0, 0, 0, 0);
        #[cfg(not(feature = "mtproto"))]
        let has_mt = false;
        #[cfg(feature = "mtproto")]
        let (has_mt, counts) = (
            self.mtproto.is_some(),
            self.mtproto.as_ref().map_or(counts, |v| v.0.counts()),
        );
        if self.schema_version != if has_mt { 2 } else { 1 }
            || self.profiles.len() + counts.0 > 64
            || self.approvals.len() + counts.1 > 256
            || self.file_restorations.len() + counts.2 > 1024
            || self.callback_observations.len() + counts.3 > 1024
        {
            return Err(invalid());
        }
        absolute(&self.journal_path)?;
        let principal = Principal::new(self.principal).map_err(|_| invalid())?;
        let policy = PolicyStore::default();
        let mut profiles = Vec::new();
        let mut names = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut approval_bounds = BTreeMap::new();
        for Strict(profile) in self.profiles {
            id(&profile.name)?;
            let bot_id = profile.bot_id.parse::<i64>().map_err(|_| invalid())?;
            if bot_id < 0
                || bot_id.to_string() != profile.bot_id
                || !names.insert(profile.name.clone())
                || !identities.insert(bot_id)
                || !valid_env(&profile.token_env)
            {
                return Err(invalid());
            }
            let grant_expiry = profile.grant.0.expires_at_ms;
            let options = profile.update_options.0.build()?;
            let grant = profile.grant.0.build(bot_id)?;
            policy
                .replace(&principal, &profile.name, grant)
                .map_err(|_| invalid())?;
            let token = std::env::var(&profile.token_env).map_err(|_| invalid())?;
            if token
                .split_once(':')
                .and_then(|(id, _)| id.parse::<i64>().ok())
                != Some(bot_id)
            {
                return Err(invalid());
            }
            let mut builder =
                tg::Bot::builder(token).allow_http_loopback(profile.allow_http_loopback);
            if let Some(url) = profile.api_url {
                builder = builder.api_url(url);
            }
            let bot = builder.build().map_err(|_| invalid())?;
            approval_bounds.insert(profile.name.clone(), (bot_id, grant_expiry));
            profiles.push(PreparedProfile {
                name: profile.name,
                bot_id,
                bot,
                options,
            });
        }
        let mut restored = BTreeSet::new();
        for Strict(restore) in &self.file_restorations {
            if !names.contains(&restore.profile)
                || !restored.insert((&restore.profile, &restore.file_handle))
            {
                return Err(invalid());
            }
            id(&restore.file_handle)?;
            absolute(&restore.path)?;
        }
        let mut confirmed = BTreeSet::new();
        let time = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_millis(),
        )
        .map_err(|_| invalid())?;
        for Strict(approval) in &self.approvals {
            validate_cursor(&approval.request).map_err(|_| invalid())?;
            if !names.contains(approval.request.profile())
                || approval.expires_at_ms <= time
                || !confirmed.insert(approval.request.fingerprint())
            {
                return Err(invalid());
            }
            let (bot_id, grant_expiry) = approval_bounds
                .get(approval.request.profile())
                .ok_or_else(invalid)?;
            if grant_expiry.is_some_and(|expiry| approval.expires_at_ms > expiry) {
                return Err(invalid());
            }
            // Pure policy validation catches decidable action/chat/file/expiry
            // errors before storage. Callback/operation scope resolves afterward.
            match policy.approve(
                &principal,
                &approval.request,
                *bot_id,
                time,
                approval.expires_at_ms,
            ) {
                Ok(_) => {}
                Err(error) if error.kind() == ExecutorErrorKind::ContextRequired => {}
                Err(_) => return Err(invalid()),
            }
        }
        let mut observed = BTreeSet::new();
        for Strict(observation) in &self.callback_observations {
            if !names.contains(&observation.profile)
                || !observation.cursor.matches_profile(&observation.profile)
                || !observed.insert((&observation.profile, observation.cursor.to_string()))
            {
                return Err(invalid());
            }
        }
        let clock = match self.callback_clock_policy {
            ClockPolicy::JournalMonotonic => CallbackClockPolicy::JournalMonotonic,
            ClockPolicy::TrustedWallClock => CallbackClockPolicy::TrustedWallClock,
        };
        #[cfg(feature = "mtproto")]
        let mtproto = self
            .mtproto
            .map(|v| v.0.prepare(&policy, &principal, &names, clock))
            .transpose()?;
        let executor = Executor::new(
            policy,
            ExecutorOptions::default().callback_clock_policy(clock),
        )
        .map_err(|_| invalid())?;
        Ok(Prepared {
            executor,
            principal,
            journal_path: self.journal_path,
            profiles,
            restorations: self.file_restorations,
            approvals: self.approvals,
            observations: self.callback_observations,
            #[cfg(feature = "mtproto")]
            mtproto,
        })
    }
}
fn valid_env(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && (name.as_bytes()[0].is_ascii_uppercase() || name.as_bytes()[0] == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
impl GrantConfig {
    fn build(self, bot_id: i64) -> Result<Grant, ToolError> {
        if self.actions.len() > 8
            || self.chats.len() > 4096
            || self.files.len() > 256
            || self.require_approval.len() > 8
            || self.costs.len() > 8
            || !unique(&self.actions)
            || !unique(&self.chats)
            || !unique(&self.files)
            || !unique(&self.require_approval)
            || (self.all_chats && !self.chats.is_empty())
        {
            return Err(invalid());
        }
        let budget = self.budget.0.build()?;
        let chats = if self.all_chats {
            ChatScope::All
        } else {
            ChatScope::only(self.chats)
        };
        let mut grant = Grant::new(bot_id, budget)
            .chats(chats)
            .files(self.files)
            .unscoped_updates(self.unscoped_updates);
        for action in self.actions {
            grant = grant.allow(action);
        }
        for action in self.require_approval {
            grant = grant.require_approval(action);
        }
        let mut costs = BTreeSet::new();
        for Strict(cost) in self.costs {
            if !costs.insert(cost.action) {
                return Err(invalid());
            }
            grant = grant.action_cost(cost.action, cost.units);
        }
        if let Some(expiry) = self.expires_at_ms {
            grant = grant.expires_at_ms(expiry);
        }
        Ok(grant)
    }
}
impl BudgetConfig {
    fn build(self) -> Result<Budget, ToolError> {
        let caps = self.limits.0;
        let budget = Budget::new(self.id)
            .map_err(|_| invalid())?
            .window(Duration::from_secs(self.window_seconds))
            .limits(BudgetLimits {
                invocations: caps.invocations,
                write_attempts: caps.write_attempts,
                upload_bytes: caps.upload_bytes,
                cost_units: caps.cost_units,
            });
        budget.validate().map_err(|_| invalid())?;
        Ok(budget)
    }
}
