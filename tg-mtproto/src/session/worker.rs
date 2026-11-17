use super::{
    Health, SessionBinding, SessionErrorKind, SessionKey, SessionOptions, Shared, View, codec,
    data::Snapshot,
    now,
    types::{Result, error, storage},
};
use rusqlite::{
    Connection, OpenFlags, TransactionBehavior, limits::Limit, params, types::ValueRef,
};
use std::{
    fs::OpenOptions as FileOptions,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, oneshot, watch};
use zeroize::Zeroize;

const APPLICATION: i64 = 0x54474d53;
type Job = Box<dyn FnOnce(&mut Native) + Send>;
type Exit = Option<std::result::Result<(), SessionErrorKind>>;
pub(super) struct Database {
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    pub shared: Arc<Shared>,
    pub binding: SessionBinding,
    pub options: SessionOptions,
    pending: Arc<Semaphore>,
    stopped: watch::Receiver<Exit>,
    clear_on_close: Arc<AtomicBool>,
}
impl Database {
    pub async fn open(
        path: PathBuf,
        binding: SessionBinding,
        key: SessionKey,
        options: SessionOptions,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::new());
        let state = shared.clone();
        let capacity = options.max_pending;
        let metadata = (binding.clone(), options.clone());
        let (sender, receiver) = mpsc::sync_channel::<Job>(capacity);
        let (ready, waiting) = oneshot::channel();
        let (exited, stopped) = watch::channel(None);
        let clear_on_close = Arc::new(AtomicBool::new(false));
        let clear = clear_on_close.clone();
        std::thread::Builder::new()
            .name("tg-mtproto-session".into())
            .spawn(move || {
                let result = match Native::open(path, binding, key, options, state.clone()) {
                    Ok(mut native) => {
                        let live = native.live();
                        if live.is_ok() {
                            state.health.send_replace(Health::Ready);
                        }
                        let _ = ready.send(live);
                        loop {
                            match receiver.recv_timeout(native.wait_duration()) {
                                Ok(job) => job(&mut native),
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                            }
                            if Instant::now() >= native.renew_at
                                && state.health() != Health::Fenced
                                && native.renew().is_err()
                            {
                                state.health.send_replace(Health::Fenced);
                            }
                        }
                        let cleared = if clear.load(Ordering::Acquire) {
                            native.clear_authorization()
                        } else {
                            Ok(())
                        };
                        let closed = native.finish();
                        cleared.and(closed)
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        Err(error)
                    }
                };
                state
                    .view
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                state.health.send_replace(if result.is_ok() {
                    Health::Closed
                } else {
                    Health::Fenced
                });
                let _ = exited.send(Some(result.map_err(|error| error.kind())));
            })
            .map_err(storage)?;
        waiting.await.map_err(storage)??;
        shared.read(|_| ())?;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            shared,
            binding: metadata.0,
            options: metadata.1,
            pending: Arc::new(Semaphore::new(capacity)),
            stopped,
            clear_on_close,
        })
    }
    pub async fn call<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Native) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.shared.read(|_| ())?;
        let permit = self
            .pending
            .clone()
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::Closed => super::error(SessionErrorKind::Closed),
                tokio::sync::TryAcquireError::NoPermits => super::error(SessionErrorKind::Capacity),
            })?;
        let (reply, waiting) = oneshot::channel();
        {
            let admission = self.sender.lock().map_err(storage)?;
            let sender = admission
                .as_ref()
                .ok_or_else(|| error(SessionErrorKind::Closed))?;
            sender
                .try_send(Box::new(move |native| {
                    let result = native
                        .live()
                        .and_then(|_| action(native))
                        .and_then(|value| {
                            native.live()?;
                            Ok(value)
                        });
                    if result.as_ref().is_err_and(|error| {
                        matches!(
                            error.kind(),
                            SessionErrorKind::Storage
                                | SessionErrorKind::Fenced
                                | SessionErrorKind::Authentication
                        )
                    }) {
                        native.shared.health.send_replace(Health::Fenced);
                    }
                    drop(permit);
                    let _ = reply.send(result);
                }))
                .map_err(storage)?;
        }
        waiting.await.map_err(storage)?
    }
    pub async fn close(&self) -> Result<()> {
        let sender = self.sender.lock().map_err(storage)?.take();
        if sender.is_some() {
            self.shared.health.send_modify(|health| {
                if *health != Health::Fenced {
                    *health = Health::Closing;
                }
            });
        }
        self.pending.close();
        drop(sender);
        let mut stopped = self.stopped.clone();
        loop {
            if let Some(result) = *stopped.borrow_and_update() {
                return result.map_err(error);
            }
            stopped.changed().await.map_err(storage)?;
        }
    }
    pub fn request_credential_clear(&self) {
        self.clear_on_close.store(true, Ordering::Release);
    }
    #[cfg(test)]
    pub fn available_slots(&self) -> usize {
        self.pending.available_permits()
    }
}

pub(super) struct Native {
    #[cfg(test)]
    expire_after_commit: bool,
    connection: Connection,
    binding: SessionBinding,
    key: SessionKey,
    pub options: SessionOptions,
    pub shared: Arc<Shared>,
    storage_id: [u8; 16],
    owner: [u8; 16],
    revision: i64,
    lease_until: i64,
    deadline: Instant,
    renew_at: Instant,
}
impl Native {
    fn open(
        path: PathBuf,
        binding: SessionBinding,
        key: SessionKey,
        options: SessionOptions,
        shared: Arc<Shared>,
    ) -> Result<Self> {
        secure_file(&path)?;
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(storage)?;
        connection
            .busy_timeout(options.busy_timeout.min(options.lease / 4))
            .map_err(storage)?;
        connection
            .set_limit(
                Limit::SQLITE_LIMIT_LENGTH,
                (options.max_snapshot_bytes + 1024) as i32,
            )
            .map_err(storage)?;
        let _ = schema(&connection)?;
        let mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(storage)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(storage(()));
        }
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(storage)?;
        connection
            .pragma_update(None, "secure_delete", "ON")
            .map_err(storage)?;
        connection
            .pragma_update(None, "trusted_schema", "OFF")
            .map_err(storage)?;
        let deadline = Instant::now()
            .checked_add(options.lease)
            .ok_or_else(|| error(SessionErrorKind::InvalidInput))?;
        let time = now()?;
        let lease_until = add_lease(time, options.lease)?;
        let mut owner = [0; 16];
        getrandom::fill(&mut owner).map_err(storage)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let fresh = schema(&transaction)?;
        let (storage_id, revision, snapshot) = if fresh {
            transaction
                .execute_batch(include_str!("schema.sql"))
                .map_err(storage)?;
            transaction
                .pragma_update(None, "application_id", APPLICATION)
                .map_err(storage)?;
            transaction
                .pragma_update(None, "user_version", 1)
                .map_err(storage)?;
            let mut id = [0; 16];
            getrandom::fill(&mut id).map_err(storage)?;
            let snapshot = Snapshot::default();
            let (nonce, ciphertext) = codec::encrypt(&snapshot, &id, 0, &binding, &key, &options)?;
            transaction.execute("INSERT INTO mtproto_session(id,format_version,storage_id,revision,nonce,ciphertext,owner,lease_until_ms) VALUES(1,1,?1,0,?2,?3,?4,?5)",params![id.as_slice(),nonce.as_slice(),ciphertext,owner.as_slice(),lease_until]).map_err(storage)?;
            (id, 0, snapshot)
        } else {
            let mut statement=transaction.prepare("SELECT format_version,storage_id,revision,nonce,ciphertext,owner,lease_until_ms FROM mtproto_session WHERE id=1").map_err(storage)?;
            let mut rows = statement.query([]).map_err(storage)?;
            let row = rows
                .next()
                .map_err(storage)?
                .ok_or_else(|| error(SessionErrorKind::Authentication))?;
            if row.get::<_, i64>(0).map_err(storage)? != 1 {
                return Err(error(SessionErrorKind::UnsupportedSchema));
            }
            let id = fixed::<16>(row.get_ref(1).map_err(storage)?)?;
            let revision: i64 = row.get(2).map_err(storage)?;
            if revision < 0 {
                return Err(error(SessionErrorKind::Authentication));
            }
            let nonce = fixed::<24>(row.get_ref(3).map_err(storage)?)?;
            let ciphertext = match row.get_ref(4).map_err(storage)? {
                ValueRef::Blob(bytes) => bytes,
                _ => return Err(error(SessionErrorKind::Authentication)),
            };
            let snapshot =
                codec::decrypt(nonce, ciphertext, &id, revision, &binding, &key, &options)?;
            let has_owner = match row.get_ref(5).map_err(storage)? {
                ValueRef::Null => false,
                value => {
                    fixed::<16>(value)?;
                    true
                }
            };
            let until: Option<i64> = row.get(6).map_err(storage)?;
            if has_owner != until.is_some() {
                return Err(error(SessionErrorKind::Authentication));
            }
            let time = now()?;
            if has_owner && until.is_some_and(|until| until > time) {
                return Err(error(SessionErrorKind::Owned));
            }
            drop(rows);
            drop(statement);
            transaction
                .execute(
                    "UPDATE mtproto_session SET owner=?1,lease_until_ms=?2 WHERE id=1",
                    params![owner.as_slice(), lease_until],
                )
                .map_err(storage)?;
            (id, revision, snapshot)
        };
        if Instant::now() >= deadline || now()? >= lease_until {
            return Err(error(SessionErrorKind::Fenced));
        }
        transaction.commit().map_err(storage)?;
        let provisional = snapshot.acknowledged.clone().unwrap_or_default();
        let renew_at = Instant::now() + options.lease / 3;
        let native = Self {
            #[cfg(test)]
            expire_after_commit: false,
            connection,
            binding,
            key,
            options,
            shared: shared.clone(),
            storage_id,
            owner,
            revision,
            lease_until,
            deadline,
            renew_at,
        };
        #[cfg(test)]
        if let Some(delay) = native.options.after_open_commit {
            std::thread::sleep(delay);
        }
        let published = (|| {
            let mut view = shared.view.write().map_err(storage)?;
            native.live()?;
            *view = Some(View {
                snapshot,
                provisional,
                lease_until_ms: lease_until,
                deadline,
            });
            drop(view);
            native.live()
        })();
        if let Err(failure) = published {
            let _ = native.finish();
            return Err(failure);
        }
        Ok(native)
    }
    pub fn live(&self) -> Result<()> {
        if self.shared.health() == Health::Fenced
            || Instant::now() >= self.deadline
            || now()? >= self.lease_until
        {
            self.shared.health.send_replace(Health::Fenced);
            Err(error(SessionErrorKind::Fenced))
        } else {
            Ok(())
        }
    }
    fn wait_duration(&self) -> Duration {
        if self.shared.health() == Health::Fenced {
            Duration::from_secs(60)
        } else {
            self.renew_at.saturating_duration_since(Instant::now())
        }
    }
    fn renew(&mut self) -> Result<()> {
        self.live()?;
        let started = Instant::now();
        let until = add_lease(now()?, self.options.lease)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if Instant::now() >= self.deadline {
            return Err(error(SessionErrorKind::Fenced));
        }
        let time = now()?;
        let changed=transaction.execute("UPDATE mtproto_session SET lease_until_ms=?1 WHERE id=1 AND owner=?2 AND revision=?3 AND lease_until_ms>?4",params![until,self.owner.as_slice(),self.revision,time]).map_err(storage)?;
        if changed != 1 {
            return Err(error(SessionErrorKind::Fenced));
        }
        transaction.commit().map_err(storage)?;
        self.live()?;
        self.deadline = started + self.options.lease;
        self.lease_until = until;
        self.renew_at = started + self.options.lease / 3;
        if Instant::now() >= self.deadline || now()? >= until {
            return Err(error(SessionErrorKind::Fenced));
        }
        if let Some(view) = self.shared.view.write().map_err(storage)?.as_mut() {
            self.live()?;
            view.deadline = self.deadline;
            view.lease_until_ms = until;
        }
        self.live()
    }
    pub fn mutate(&mut self, change: impl FnOnce(&mut Snapshot) -> Result<()>) -> Result<()> {
        self.live()?;
        let mut next = self
            .shared
            .view
            .read()
            .map_err(storage)?
            .as_ref()
            .ok_or_else(|| error(SessionErrorKind::Closed))?
            .snapshot
            .clone();
        change(&mut next)?;
        if next
            == self
                .shared
                .view
                .read()
                .map_err(storage)?
                .as_ref()
                .ok_or_else(|| error(SessionErrorKind::Closed))?
                .snapshot
        {
            return Ok(());
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| error(SessionErrorKind::Capacity))?;
        let (nonce, ciphertext) = codec::encrypt(
            &next,
            &self.storage_id,
            revision,
            &self.binding,
            &self.key,
            &self.options,
        )?;
        self.live()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if Instant::now() >= self.deadline {
            return Err(error(SessionErrorKind::Fenced));
        }
        let time = now()?;
        let changed=transaction.execute("UPDATE mtproto_session SET revision=?1,nonce=?2,ciphertext=?3 WHERE id=1 AND owner=?4 AND revision=?5 AND lease_until_ms>?6",params![revision,nonce.as_slice(),ciphertext,self.owner.as_slice(),self.revision,time]).map_err(storage)?;
        if changed != 1 {
            self.shared.health.send_replace(Health::Fenced);
            return Err(error(SessionErrorKind::Fenced));
        }
        transaction.commit().map_err(storage)?;
        self.revision = revision;
        #[cfg(test)]
        if self.expire_after_commit {
            self.deadline = Instant::now();
            self.expire_after_commit = false;
        }
        let mut view = self.shared.view.write().map_err(storage)?;
        self.live()?;
        view.as_mut()
            .ok_or_else(|| error(SessionErrorKind::Closed))?
            .snapshot = next;
        drop(view);
        self.live()
    }
    pub fn clear_authorization(&mut self) -> Result<()> {
        self.mutate(|snapshot| {
            if !snapshot.quarantined {
                return Err(error(SessionErrorKind::InvalidInput));
            }
            for dc in &mut snapshot.datacenters {
                if let Some(key) = &mut dc.auth_key {
                    key.zeroize();
                }
                dc.auth_key = None;
            }
            snapshot.peers.clear();
            snapshot.account = None;
            snapshot.acknowledged = None;
            snapshot.updates.clear_authorization();
            snapshot.quarantined = false;
            Ok(())
        })?;
        let mut view = self.shared.view.write().map_err(storage)?;
        view.as_mut()
            .ok_or_else(|| error(SessionErrorKind::Closed))?
            .provisional = Default::default();
        self.live()
    }
    #[cfg(test)]
    pub fn expire_on_commit(&mut self) {
        self.expire_after_commit = true;
    }
    fn finish(self) -> Result<()> {
        let prior = if self.shared.health() == Health::Fenced {
            Err(error(SessionErrorKind::Fenced))
        } else {
            Ok(())
        };
        let released = self
            .connection
            .execute(
                "UPDATE mtproto_session SET owner=NULL,lease_until_ms=NULL WHERE id=1 AND owner=?1",
                [self.owner.as_slice()],
            )
            .map_err(storage);
        self.shared
            .view
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(self.key);
        let closed = self.connection.close().map_err(storage);
        prior.and(released.map(|_| ())).and(closed)
    }
}

fn fixed<const N: usize>(value: ValueRef<'_>) -> Result<[u8; N]> {
    match value {
        ValueRef::Blob(bytes) => bytes
            .try_into()
            .map_err(|_| error(SessionErrorKind::Authentication)),
        _ => Err(error(SessionErrorKind::Authentication)),
    }
}
fn add_lease(time: i64, duration: Duration) -> Result<i64> {
    time.checked_add(i64::try_from(duration.as_millis()).map_err(storage)?)
        .ok_or_else(|| error(SessionErrorKind::InvalidInput))
}
fn schema(connection: &Connection) -> Result<bool> {
    let application: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(storage)?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(storage)?;
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT GLOB 'sqlite_*'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if application == 0 && version == 0 && count == 0 {
        return Ok(true);
    }
    if application != APPLICATION || version != 1 || count != 1 {
        return Err(error(SessionErrorKind::UnsupportedSchema));
    }
    let actual: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='mtproto_session'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if actual.trim() != include_str!("schema.sql").trim().trim_end_matches(';') {
        return Err(error(SessionErrorKind::UnsupportedSchema));
    }
    Ok(false)
}
fn secure_file(path: &Path) -> Result<()> {
    let mut options = FileOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path).map_err(storage)?;
            if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                Err(error(SessionErrorKind::InvalidInput))
            } else {
                Ok(())
            }
        }
        Err(_) => Err(storage(())),
    }
}
