use super::{ExecutorError, ExecutorErrorKind, Principal, runtime::storage_error, types::valid_id};
use crate::journal::{self, Journal, JournalError, JournalErrorKind};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
type Result<T> = std::result::Result<T, ExecutorError>;
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
    pub(crate) fn validate(&self) -> Result<()> {
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
pub(crate) struct FileRegistry {
    entries: std::sync::Mutex<BTreeMap<String, Arc<Snapshot>>>,
    memory: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    uploads: Arc<Semaphore>,
    limits: FileLimits,
}
impl FileRegistry {
    pub(crate) fn new(limits: FileLimits) -> Self {
        Self {
            limits,
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
    pub(crate) fn upload(&self, file: &RegisteredFile) -> Result<tg::InputFile> {
        Ok(tg::InputFile::owned_bytes(
            file.filename.clone(),
            self.upload_owner(file)?,
        ))
    }
    pub(crate) fn upload_owner(&self, file: &RegisteredFile) -> Result<Upload> {
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
        Ok(Upload {
            snapshot,
            _permit: permit,
        })
    }
}
pub(crate) struct Upload {
    snapshot: Arc<Snapshot>,
    _permit: OwnedSemaphorePermit,
}
impl AsRef<[u8]> for Upload {
    fn as_ref(&self) -> &[u8] {
        self.snapshot.as_ref().as_ref()
    }
}
impl FileRegistry {
    /// Trusted-host registration. This creates no agent grant or approval.
    pub(crate) async fn register_file(
        &self,
        journal: &Journal,
        principal: &Principal,
        profile: &str,
        filename: impl Into<String>,
        bytes: Vec<u8>,
        expires_at_ms: i64,
    ) -> Result<RegisteredFile> {
        let filename = filename.into();
        validate_filename(&filename)?;
        ensure_live(expires_at_ms, now()?)?;
        let input = self.accept(bytes, self.limits)?;
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
        let file = journal
            .insert_file_metadata(file)
            .await
            .map_err(file_storage)?;
        ensure_live(file.expires_at_ms, now()?)?;
        self.install(file.clone(), input)?;
        Ok(file)
    }
    /// Reattaches only identical owned bytes; immutable metadata is never replaced.
    pub(crate) async fn restore_file(
        &self,
        journal: &Journal,
        principal: &Principal,
        profile: &str,
        handle: &str,
        bytes: Vec<u8>,
    ) -> Result<RegisteredFile> {
        valid_id(handle)?;
        let input = self.accept(bytes, self.limits)?;
        let file = journal
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
        self.install(file.clone(), input)?;
        Ok(file)
    }
    /// Discards resident bytes. Outstanding owners retain their memory permits.
    pub(crate) fn discard_file(
        &self,
        principal: &Principal,
        profile: &str,
        handle: &str,
    ) -> Result<bool> {
        valid_id(profile)?;
        valid_id(handle)?;
        let mut entries = self.entries.lock().map_err(|_| invalid_policy())?;
        if entries.get(handle).is_some_and(|snapshot| {
            snapshot.file.profile == profile && snapshot.file.principal == principal.as_str()
        }) {
            entries.remove(handle);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub(crate) fn prune_files(&self) -> Result<usize> {
        let time = now()?;
        let mut entries = self.entries.lock().map_err(|_| invalid_policy())?;
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
pub(crate) fn validate_filename(name: &str) -> Result<()> {
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
pub(crate) fn ensure_live(expiry: i64, time: i64) -> Result<()> {
    if expiry <= time {
        return Err(ExecutorError::new(ExecutorErrorKind::FileExpired));
    }
    Ok(())
}
pub(crate) fn remaining(expiry: i64, time: i64) -> Result<Duration> {
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

fn now() -> Result<i64> {
    journal::now().map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
