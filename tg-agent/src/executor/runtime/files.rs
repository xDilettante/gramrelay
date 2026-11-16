use super::*;
pub use crate::journal::files::RegisteredFile;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;

/// Trusted-host memory limits, independent of per-grant durable upload quotas.
#[derive(Debug, Clone, Copy)]
pub struct FileLimits {
    pub max_file_bytes: u64,
    pub max_snapshot_bytes: u64,
    pub max_snapshots: usize,
    pub max_upload_bytes: u64,
}
impl Default for FileLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 50 * 1024 * 1024,
            max_snapshot_bytes: 100 * 1024 * 1024,
            max_snapshots: 1024,
            max_upload_bytes: 50 * 1024 * 1024,
        }
    }
}
impl FileLimits {
    pub(super) fn validate(&self) -> Result<()> {
        if self.max_file_bytes == 0
            || self.max_file_bytes > self.max_snapshot_bytes
            || self.max_snapshot_bytes > 1024 * 1024 * 1024
            || self.max_snapshot_bytes > Semaphore::MAX_PERMITS as u64
            || self.max_snapshots == 0
            || self.max_snapshots > 65536
            || self.max_upload_bytes == 0
            || self.max_upload_bytes > 1024 * 1024 * 1024
            || self.max_upload_bytes > Semaphore::MAX_PERMITS as u64
        {
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy));
        }
        Ok(())
    }
}
struct OwnedInput {
    bytes: Vec<u8>,
    _memory: OwnedSemaphorePermit,
    _slot: OwnedSemaphorePermit,
}
struct Snapshot {
    file: RegisteredFile,
    data: OwnedInput,
}
impl AsRef<[u8]> for Snapshot {
    fn as_ref(&self) -> &[u8] {
        &self.data.bytes
    }
}
pub(super) struct FileRegistry {
    entries: std::sync::Mutex<BTreeMap<String, Arc<Snapshot>>>,
    memory: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    uploads: Arc<Semaphore>,
}
impl FileRegistry {
    pub(super) fn new(limits: FileLimits) -> Self {
        Self {
            entries: Default::default(),
            memory: Arc::new(Semaphore::new(limits.max_snapshot_bytes as usize)),
            slots: Arc::new(Semaphore::new(limits.max_snapshots)),
            uploads: Arc::new(Semaphore::new(limits.max_upload_bytes as usize)),
        }
    }
    fn accept(&self, bytes: Vec<u8>, limits: FileLimits) -> Result<OwnedInput> {
        if bytes.len() as u64 > limits.max_file_bytes {
            return Err(ExecutorError::new(ExecutorErrorKind::Capacity));
        }
        // Charge retained allocation, not just the transmitted prefix.
        let retained_bytes = u32::try_from(bytes.capacity()).map_err(|_| capacity())?;
        let memory = self
            .memory
            .clone()
            .try_acquire_many_owned(retained_bytes)
            .map_err(|_| capacity())?;
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| capacity())?;
        Ok(OwnedInput {
            bytes,
            _memory: memory,
            _slot: slot,
        })
    }
    fn install(&self, file: RegisteredFile, data: OwnedInput) -> Result<()> {
        let mut entries = self.entries.lock().map_err(|_| invalid_policy())?;
        if entries.contains_key(file.handle()) {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        entries.insert(file.handle.clone(), Arc::new(Snapshot { file, data }));
        Ok(())
    }
    pub(super) fn upload(&self, file: &RegisteredFile) -> Result<tg::InputFile> {
        ensure_live(file.expires_at_ms, now()?)?;
        let snapshot = self
            .entries
            .lock()
            .map_err(|_| invalid_policy())?
            .get(file.handle())
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::FileUnavailable))?;
        if snapshot.file != *file {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        let permit = self
            .uploads
            .clone()
            .try_acquire_many_owned(u32::try_from(file.byte_size).map_err(|_| capacity())?)
            .map_err(|_| capacity())?;
        Ok(tg::InputFile::owned_bytes(
            file.filename.clone(),
            Upload {
                snapshot,
                _permit: permit,
            },
        ))
    }
}
struct Upload {
    snapshot: Arc<Snapshot>,
    _permit: OwnedSemaphorePermit,
}
impl AsRef<[u8]> for Upload {
    fn as_ref(&self) -> &[u8] {
        self.snapshot.as_ref().as_ref()
    }
}
impl Executor {
    fn file_profile(&self, profile: &str) -> Result<Arc<Profile>> {
        valid_id(profile)?;
        self.profiles
            .read()
            .map_err(|_| invalid_policy())?
            .get(profile)
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Denied))
    }
    /// Trusted-host registration. This creates no agent grant or approval.
    pub async fn register_file(
        &self,
        principal: &Principal,
        profile: &str,
        filename: impl Into<String>,
        bytes: Vec<u8>,
        expires_at_ms: i64,
    ) -> Result<RegisteredFile> {
        let source = self.file_profile(profile)?;
        let filename = filename.into();
        validate_filename(&filename)?;
        ensure_live(expires_at_ms, now()?)?;
        let input = self.files.accept(bytes, self.options.file_limits)?;
        let (input, sha256) = hash(input).await?;
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(|_| ExecutorError::new(ExecutorErrorKind::Storage))?;
        let file = RegisteredFile {
            handle: format!("file_{}", URL_SAFE_NO_PAD.encode(random)),
            profile: profile.to_owned(),
            principal: principal.as_str().to_owned(),
            sha256,
            byte_size: input.bytes.len() as u64,
            filename,
            expires_at_ms,
        };
        let file = source
            .journal
            .insert_file_metadata(file)
            .await
            .map_err(file_storage)?;
        ensure_live(file.expires_at_ms, now()?)?;
        self.files.install(file.clone(), input)?;
        Ok(file)
    }
    /// Reattaches only identical owned bytes; immutable metadata is never replaced.
    pub async fn restore_file(
        &self,
        principal: &Principal,
        profile: &str,
        handle: &str,
        bytes: Vec<u8>,
    ) -> Result<RegisteredFile> {
        valid_id(handle)?;
        let source = self.file_profile(profile)?;
        let input = self.files.accept(bytes, self.options.file_limits)?;
        let file = source
            .journal
            .file_metadata(profile, principal.as_str(), handle)
            .await
            .map_err(file_storage)?
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
        ensure_live(file.expires_at_ms, now()?)?;
        if input.bytes.len() as u64 != file.byte_size {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        let (input, hash) = hash(input).await?;
        if hash != file.sha256 {
            return Err(ExecutorError::new(ExecutorErrorKind::Conflict));
        }
        ensure_live(file.expires_at_ms, now()?)?;
        self.files.install(file.clone(), input)?;
        Ok(file)
    }
    /// Discards resident bytes. Outstanding owners retain their memory permits.
    pub fn discard_file(&self, principal: &Principal, profile: &str, handle: &str) -> Result<bool> {
        valid_id(profile)?;
        valid_id(handle)?;
        let mut entries = self.files.entries.lock().map_err(|_| invalid_policy())?;
        if entries.get(handle).is_some_and(|snapshot| {
            snapshot.file.profile == profile && snapshot.file.principal == principal.as_str()
        }) {
            entries.remove(handle);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn prune_files(&self) -> Result<usize> {
        let time = now()?;
        let mut entries = self.files.entries.lock().map_err(|_| invalid_policy())?;
        let before = entries.len();
        entries.retain(|_, snapshot| snapshot.file.expires_at_ms > time);
        Ok(before - entries.len())
    }
}
async fn hash(input: OwnedInput) -> Result<(OwnedInput, [u8; 32])> {
    // Permits travel with the native job and its result if the async waiter cancels.
    tokio::task::spawn_blocking(move || {
        let hash = Sha256::digest(&input.bytes).into();
        (input, hash)
    })
    .await
    .map_err(|_| ExecutorError::new(ExecutorErrorKind::Storage))
}
fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(ExecutorError::new(ExecutorErrorKind::InvalidInput));
    }
    Ok(())
}
pub(super) fn ensure_live(expiry: i64, time: i64) -> Result<()> {
    if expiry <= time {
        return Err(ExecutorError::new(ExecutorErrorKind::FileExpired));
    }
    Ok(())
}
pub(super) fn remaining(expiry: i64, time: i64) -> Result<Duration> {
    expiry
        .checked_sub(time)
        .filter(|v| *v > 0)
        .and_then(|v| u64::try_from(v).ok())
        .map(Duration::from_millis)
        .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::FileExpired))
}
fn file_storage(error: JournalError) -> ExecutorError {
    if error.kind() == JournalErrorKind::Expired {
        ExecutorError::new(ExecutorErrorKind::FileExpired)
    } else {
        storage_error(error)
    }
}
fn invalid_policy() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
}
fn capacity() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::Capacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{journal::JournalOptions, updates::UpdateOptions};
    use std::{future::Future, task::Poll};

    #[test]
    fn an_upload_owner_keeps_snapshot_and_active_capacity_after_registry_discard() {
        let limits = FileLimits {
            max_file_bytes: 3,
            max_snapshot_bytes: 3,
            max_snapshots: 1,
            max_upload_bytes: 3,
        };
        let registry = FileRegistry::new(limits);
        let data = registry.accept(vec![1, 2, 3], limits).unwrap();
        let file = RegisteredFile {
            handle: "file_one".into(),
            profile: "primary".into(),
            principal: "alice".into(),
            sha256: Sha256::digest([1, 2, 3]).into(),
            byte_size: 3,
            filename: "one.bin".into(),
            expires_at_ms: now().unwrap() + 60000,
        };
        registry.install(file.clone(), data).unwrap();
        let upload = registry.upload(&file).unwrap();
        registry.entries.lock().unwrap().remove(file.handle());
        assert_eq!(registry.memory.available_permits(), 0);
        assert_eq!(registry.slots.available_permits(), 0);
        assert_eq!(registry.uploads.available_permits(), 0);
        assert!(registry.accept(vec![1], limits).is_err());
        drop(upload);
        assert_eq!(registry.memory.available_permits(), 3);
        assert_eq!(registry.slots.available_permits(), 1);
        assert_eq!(registry.uploads.available_permits(), 3);
        registry.accept(vec![1], limits).unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pending_restore_reserves_snapshot_memory_before_waiting_for_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("files.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let limits = FileLimits {
            max_file_bytes: 1,
            max_snapshot_bytes: 1,
            max_snapshots: 1,
            max_upload_bytes: 1,
        };
        let executor = Executor::new(
            PolicyStore::default(),
            ExecutorOptions::default().file_limits(limits),
        )
        .unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                tg::test_kit::MockTransport::new([]).unwrap().bot().unwrap(),
            )
            .unwrap();
        let principal = Principal::new("alice").unwrap();
        let file = executor
            .register_file(
                &principal,
                "primary",
                "one.bin",
                vec![1],
                now().unwrap() + 60000,
            )
            .await
            .unwrap();
        executor
            .discard_file(&principal, "primary", file.handle())
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked = journal.clone();
        let gate = tokio::spawn(async move {
            blocked
                .run(move |_, _, _| {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        let first = executor.restore_file(&principal, "primary", file.handle(), vec![1]);
        tokio::pin!(first);
        std::future::poll_fn(|cx| match first.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("metadata must wait for database gate"),
        })
        .await;
        let second = timeout(
            Duration::from_millis(40),
            executor.restore_file(&principal, "primary", file.handle(), vec![1]),
        )
        .await;
        release_tx.send(()).unwrap();
        gate.await.unwrap().unwrap();
        first.await.unwrap();
        assert_eq!(
            second
                .expect("pending restore must reserve capacity before metadata wait")
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Capacity
        );
        journal.close().await.unwrap();
    }

    #[tokio::test]
    async fn expired_document_cannot_start_identity_after_waiting_for_another_initializer() {
        use tg::test_kit::{MockResponse, MockTransport, Step};
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("identity.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let policy = PolicyStore::default();
        let principal = Principal::new("alice").unwrap();
        let mock=MockTransport::new([Step::reply("getMe",MockResponse::json(serde_json::json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}})))]).unwrap();
        let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                mock.bot().unwrap(),
            )
            .unwrap();
        let file = executor
            .register_file(
                &principal,
                "primary",
                "one.bin",
                vec![1],
                now().unwrap() + 1000,
            )
            .await
            .unwrap();
        policy
            .replace(
                &principal,
                "primary",
                Grant::new(0, Budget::new("quota").unwrap())
                    .allow(Action::SendDocument)
                    .chats(ChatScope::only([ChatId::new(1).unwrap()]))
                    .files([file.handle()]),
            )
            .unwrap();
        let request=Request::from_json(&serde_json::to_vec(&serde_json::json!({"schema_version":1,"profile":"primary","operation_id":"one","call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":file.handle()}}})).unwrap()).unwrap();
        let profile = executor.file_profile("primary").unwrap();
        let auth = executor
            .read_authorization(&profile, &principal, &request, None)
            .await
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let held = profile.clone();
        let gate = tokio::spawn(async move {
            let _ = held
                .identity
                .get_or_try_init(|| async {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    Err::<BotInfo, _>(ExecutorError::new(ExecutorErrorKind::Identity))
                })
                .await;
        });
        started_rx.await.unwrap();
        let verified = executor.verified(&profile, &auth);
        tokio::pin!(verified);
        std::future::poll_fn(|cx| match verified.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("identity must wait"),
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        release_tx.send(()).unwrap();
        gate.await.unwrap();
        let result = verified.await;
        assert!(
            mock.requests().unwrap().is_empty(),
            "expired document must not start getMe"
        );
        let Err(error) = result else {
            panic!("expired file must fail")
        };
        assert_eq!(error.kind(), ExecutorErrorKind::FileExpired);
        journal.close().await.unwrap();
    }
}
