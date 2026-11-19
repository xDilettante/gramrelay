use super::*;
use crate::{
    mtproto::{self as mt, Action as MtAction, PeerId, PeerScope},
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, Client, ClientOptions, Session, SessionBinding,
    SessionKey, SessionOptions,
};
use zeroize::Zeroizing;

pub(super) type Factory =
    Box<dyn FnMut(Session, ClientOptions) -> Result<Client, tg_mtproto::ClientError> + Send>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    profiles: Vec<Strict<Profile>>,
    #[serde(default)]
    approvals: Vec<Strict<Confirmation>>,
    #[serde(default)]
    file_restorations: Vec<Strict<Restoration>>,
    #[serde(default)]
    callback_observations: Vec<Strict<Observation>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    account_kind: AccountKind,
    account_id: String,
    session_path: PathBuf,
    session_binding: String,
    api_id: i32,
    session_key_env: String,
    grant: Strict<MtGrant>,
    #[serde(default)]
    update_options: Strict<FeedConfig>,
    #[serde(default = "timeout_ms")]
    request_timeout_ms: u64,
    #[serde(default = "pending")]
    max_pending: usize,
}
fn timeout_ms() -> u64 {
    30000
}
fn pending() -> usize {
    16
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MtGrant {
    #[serde(default)]
    actions: Vec<MtAction>,
    #[serde(default)]
    peers: Vec<PeerId>,
    #[serde(default)]
    all_peers: bool,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    unscoped_updates: bool,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    #[serde(default)]
    require_approval: Vec<MtAction>,
    #[serde(default)]
    costs: Vec<Strict<Cost>>,
    budget: Strict<BudgetConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cost {
    action: MtAction,
    units: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    request: mt::Request,
    expires_at_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    profile: String,
    cursor: mt::updates::Cursor,
    reference: String,
}
struct PreparedProfile {
    name: String,
    expected: ProfileIdentity,
    identity: AccountIdentity,
    path: PathBuf,
    binding: SessionBinding,
    key: SessionKey,
    client_options: ClientOptions,
    feed_options: UpdateOptions,
}
pub(super) struct Prepared {
    profiles: Vec<PreparedProfile>,
    executor: mt::Executor,
    approvals: Vec<Strict<Confirmation>>,
    restorations: Vec<Strict<Restoration>>,
    observations: Vec<Strict<Observation>>,
}
struct LoadedProfile {
    name: String,
    expected: ProfileIdentity,
    feed_options: UpdateOptions,
}
pub(super) struct Loaded {
    pub resources: Resources,
    profiles: Vec<LoadedProfile>,
    approvals: Vec<Strict<Confirmation>>,
    restorations: Vec<Strict<Restoration>>,
    observations: Vec<Strict<Observation>>,
}
pub(super) struct Resources {
    executor: mt::Executor,
    clients: Vec<Client>,
}
impl Resources {
    pub async fn close(&self) -> Result<(), ToolError> {
        let mut failed = self.executor.close().await.is_err();
        for client in &self.clients {
            failed |= client.close().await.is_err();
        }
        if failed {
            Err(ToolError::new(
                "agent_mtproto_close",
                "MTProto host close failed.",
            ))
        } else {
            Ok(())
        }
    }
}
impl Config {
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.profiles.len(),
            self.approvals.len(),
            self.file_restorations.len(),
            self.callback_observations.len(),
        )
    }
    pub fn prepare(
        self,
        policy: &PolicyStore,
        principal: &Principal,
        bot_names: &BTreeSet<String>,
        clock: CallbackClockPolicy,
    ) -> Result<Prepared, ToolError> {
        if self.profiles.is_empty() {
            return Err(invalid());
        }
        let mut names = bot_names.clone();
        let mut expected = BTreeMap::new();
        let mut identities = BTreeSet::new();
        let mut profiles = Vec::new();
        for Strict(profile) in self.profiles {
            id(&profile.name)?;
            absolute(&profile.session_path)?;
            let account_id = profile.account_id.parse::<i64>().map_err(|_| invalid())?;
            let account = AccountId::new(account_id).map_err(|_| invalid())?;
            let (identity, kind) = match profile.account_kind {
                AccountKind::Bot => (ProfileIdentity::mtproto_bot(account_id), 0u8),
                AccountKind::User => (ProfileIdentity::mtproto_user(account_id), 1u8),
                _ => return Err(invalid()),
            };
            let identity = identity.map_err(|_| invalid())?;
            if profile.account_id != account_id.to_string()
                || !names.insert(profile.name.clone())
                || !identities.insert((kind, account_id))
                || !valid_env(&profile.session_key_env)
                || !(1..=30000).contains(&profile.request_timeout_ms)
                || !(1..=1024).contains(&profile.max_pending)
            {
                return Err(invalid());
            }
            let binding = SessionBinding::new(profile.session_binding, profile.api_id)
                .map_err(|_| invalid())?;
            let grant = profile.grant.0.build(identity)?;
            let expiry = grant.expiry();
            policy
                .replace_mtproto(principal, &profile.name, grant)
                .map_err(|_| invalid())?;
            let key = environment_key(&profile.session_key_env)?;
            let feed_options = profile.update_options.0.build()?;
            expected.insert(profile.name.clone(), (identity, expiry));
            profiles.push(PreparedProfile {
                name: profile.name,
                expected: identity,
                identity: AccountIdentity {
                    id: account,
                    kind: profile.account_kind,
                },
                path: profile.session_path,
                binding,
                key,
                client_options: ClientOptions::default()
                    .request_timeout(Duration::from_millis(profile.request_timeout_ms))
                    .max_pending(profile.max_pending),
                feed_options,
            });
        }
        let mut restored = BTreeSet::new();
        for Strict(restore) in &self.file_restorations {
            if !expected.contains_key(&restore.profile)
                || !restored.insert((&restore.profile, &restore.file_handle))
            {
                return Err(invalid());
            }
            id(&restore.file_handle)?;
            absolute(&restore.path)?;
        }
        let mut observed = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        for Strict(observation) in &self.callback_observations {
            if !expected.contains_key(&observation.profile)
                || !observation.cursor.matches_profile(&observation.profile)
                || !observed.insert((&observation.profile, observation.cursor.to_string()))
                || !aliases.insert(&observation.reference)
                || !mt::CallbackReference::valid_alias(&observation.reference)
            {
                return Err(invalid());
            }
        }
        let time = crate::journal::now().map_err(|_| invalid())?;
        let mut confirmed = BTreeSet::new();
        for Strict(approval) in &self.approvals {
            let (identity, expiry) = expected
                .get(approval.request.profile())
                .ok_or_else(invalid)?;
            if approval.expires_at_ms <= time
                || expiry.is_some_and(|v| approval.expires_at_ms > v)
                || !confirmed.insert(approval.request.fingerprint())
            {
                return Err(invalid());
            }
            if let mt::Call::ReadUpdates(args) = approval.request.call()
                && let Some(cursor) = &args.cursor
            {
                let cursor: mt::updates::Cursor = cursor.parse().map_err(|_| invalid())?;
                if !cursor.matches_profile(approval.request.profile()) {
                    return Err(invalid());
                }
            }
            if let mt::Call::AnswerCallbackQuery(args) = approval.request.call()
                && !self.callback_observations.iter().any(|v| {
                    v.0.profile == approval.request.profile() && v.0.reference == args.reference
                })
            {
                return Err(invalid());
            }
            match policy.approve_mtproto(
                principal,
                &approval.request,
                *identity,
                time,
                approval.expires_at_ms,
            ) {
                Ok(_) => {}
                Err(error) if error.kind() == ExecutorErrorKind::ContextRequired => {}
                Err(_) => return Err(invalid()),
            }
        }
        let executor = mt::Executor::new(
            policy.clone(),
            mt::ExecutorOptions::default().callback_clock_policy(clock),
        )
        .map_err(|_| invalid())?;
        Ok(Prepared {
            profiles,
            executor,
            approvals: self.approvals,
            restorations: self.file_restorations,
            observations: self.callback_observations,
        })
    }
}
fn environment_key(name: &str) -> Result<SessionKey, ToolError> {
    let encoded = Zeroizing::new(std::env::var(name).map_err(|_| invalid())?);
    if encoded.len() != 64 || !encoded.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(SessionKey::from_bytes(*bytes))
}
impl MtGrant {
    fn build(self, expected: ProfileIdentity) -> Result<mt::Grant, ToolError> {
        if self.actions.len() > 8
            || self.peers.len() > 4096
            || self.files.len() > 256
            || self.require_approval.len() > 8
            || self.costs.len() > 8
            || !unique(&self.actions)
            || !unique(&self.peers)
            || !unique(&self.files)
            || !unique(&self.require_approval)
            || (self.all_peers && !self.peers.is_empty())
        {
            return Err(invalid());
        }
        let peers = if self.all_peers {
            PeerScope::All
        } else {
            PeerScope::only(self.peers)
        };
        let mut grant = mt::Grant::new(expected, self.budget.0.build()?)
            .map_err(|_| invalid())?
            .peers(peers)
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
impl Prepared {
    pub async fn open(
        mut self,
        journal_path: &Path,
        mut factory: Factory,
    ) -> Result<Loaded, ToolError> {
        let journal = match tokio::fs::canonicalize(journal_path).await {
            Ok(v) => Some(v),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(invalid()),
        };
        let mut paths = BTreeSet::new();
        for profile in &mut self.profiles {
            let metadata = tokio::fs::symlink_metadata(&profile.path)
                .await
                .map_err(|_| invalid())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid());
            }
            let path = tokio::fs::canonicalize(&profile.path)
                .await
                .map_err(|_| invalid())?;
            if journal.as_ref() == Some(&path) || !paths.insert(path.clone()) {
                return Err(invalid());
            }
            profile.path = path;
        }
        let mut files = self
            .profiles
            .iter()
            .map(|p| p.path.clone())
            .collect::<Vec<_>>();
        files.extend(journal);
        reject_file_aliases(files).await?;
        let mut resources = Resources {
            executor: self.executor,
            clients: Vec::new(),
        };
        let mut profiles = Vec::new();
        let opened = async {
            for profile in self.profiles {
                let session = Session::open_existing(
                    profile.path,
                    profile.binding,
                    profile.key,
                    SessionOptions::default(),
                )
                .await
                .map_err(|_| invalid())?;
                let valid=matches!((session.remembered_account(),session.is_quarantined()),(Ok(Some(identity)),Ok(false)) if identity==profile.identity);
                if !valid {
                    let _ = session.close().await;
                    return Err(invalid());
                }
                let client = match factory(session.clone(), profile.client_options) {
                    Ok(client) => client,
                    Err(_) => {
                        let _ = session.close().await;
                        return Err(invalid());
                    }
                };
                resources.clients.push(client);
                profiles.push(LoadedProfile {
                    name: profile.name,
                    expected: profile.expected,
                    feed_options: profile.feed_options,
                });
            }
            Ok(())
        }
        .await;
        if let Err(error) = opened {
            let _ = resources.close().await;
            return Err(error);
        }
        Ok(Loaded {
            resources,
            profiles,
            approvals: self.approvals,
            restorations: self.restorations,
            observations: self.observations,
        })
    }
}
async fn reject_file_aliases(paths: Vec<PathBuf>) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut handles = Vec::with_capacity(paths.len());
        for path in paths {
            let handle = same_file::Handle::from_path(path).map_err(|_| invalid())?;
            if handles.contains(&handle) {
                return Err(invalid());
            }
            handles.push(handle);
        }
        Ok(())
    })
    .await
    .map_err(|_| invalid())?
}
impl Loaded {
    pub async fn attach(
        &mut self,
        journal: &Journal,
        principal: &Principal,
        session: ToolSession,
    ) -> Result<ToolSession, ToolError> {
        let mut feeds = BTreeMap::new();
        for (profile, client) in self.profiles.iter().zip(&self.resources.clients) {
            let feed = mt::updates::Feed::open(
                journal.clone(),
                &profile.name,
                profile.expected,
                profile.feed_options.clone(),
            )
            .await
            .map_err(|_| invalid())?;
            self.resources
                .executor
                .register_profile(feed.clone(), client.clone())
                .map_err(|_| invalid())?;
            feeds.insert(profile.name.clone(), feed);
        }
        for Strict(observation) in &self.observations {
            let event = feeds
                .get(&observation.profile)
                .ok_or_else(invalid)?
                .event_at(&observation.cursor)
                .await
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            self.resources
                .executor
                .observe_callback_as(
                    principal,
                    &observation.profile,
                    &event,
                    &observation.reference,
                )
                .map_err(|_| invalid())?;
        }
        for Strict(restore) in &self.restorations {
            let bytes = read_regular(&restore.path, FileLimits::default().max_file_bytes).await?;
            self.resources
                .executor
                .restore_file(principal, &restore.profile, &restore.file_handle, bytes)
                .await
                .map_err(|_| invalid())?;
        }
        let session = session.with_mtproto(self.resources.executor.clone());
        for Strict(approval) in &self.approvals {
            session
                .approve_mtproto(&approval.request, approval.expires_at_ms)
                .await
                .map_err(|_| invalid())?;
        }
        Ok(session)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_file_identity_preflight_refuses_hard_links_but_accepts_distinct_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        let alias = dir.path().join("alias");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        std::fs::hard_link(&first, &alias).unwrap();
        reject_file_aliases(vec![first.clone(), second])
            .await
            .unwrap();
        assert!(reject_file_aliases(vec![first, alias]).await.is_err());
    }
}
