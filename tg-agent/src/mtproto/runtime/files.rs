use super::*;
impl Executor {
    fn file_profile(&self, profile: &str) -> Result<Arc<Profile>> {
        crate::executor::valid_id(profile)?;
        let jobs = self.jobs.try_lock().map_err(|_| capacity())?;
        if jobs.closed {
            return Err(closed());
        }
        self.profiles
            .read()
            .map_err(|_| invalid_policy())?
            .get(profile)
            .cloned()
            .ok_or_else(denied)
    }
    /// Trusted host bytes only. Registration creates no grant or approval.
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
        validate(&filename, bytes.len() as u64)?;
        self.files
            .register_file(
                &source.journal,
                principal,
                profile,
                filename,
                bytes,
                expires_at_ms,
            )
            .await
    }
    /// Restores identical bytes under the original owner/profile/hash/size only.
    pub async fn restore_file(
        &self,
        principal: &Principal,
        profile: &str,
        handle: &str,
        bytes: Vec<u8>,
    ) -> Result<RegisteredFile> {
        let source = self.file_profile(profile)?;
        if bytes.is_empty() {
            return Err(invalid());
        }
        let file = self
            .files
            .restore_file(&source.journal, principal, profile, handle, bytes)
            .await?;
        if let Err(error) = validate_metadata(&file) {
            self.files.discard_file(principal, profile, handle)?;
            return Err(error);
        }
        Ok(file)
    }
    /// Existing upload owners retain both resident and active-upload permits.
    pub fn discard_file(&self, principal: &Principal, profile: &str, handle: &str) -> Result<bool> {
        self.files.discard_file(principal, profile, handle)
    }
    pub fn prune_files(&self) -> Result<usize> {
        self.files.prune_files()
    }
}
fn invalid() -> ExecutorError {
    ExecutorError::new(ExecutorErrorKind::InvalidInput)
}
fn validate(name: &str, size: u64) -> Result<()> {
    file_store::validate_filename(name)?;
    if size == 0 || name.ends_with(['.', ' ']) || name.chars().any(|c| ":<>\"|?*".contains(c)) {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn validate_metadata(file: &RegisteredFile) -> Result<()> {
    validate(&file.filename, file.byte_size)
}
pub(super) fn cap(
    file: &RegisteredFile,
    anchor: Instant,
    time: i64,
    budget: Duration,
) -> Result<Instant> {
    // Clamp before Instant conversion: a valid distant UTC expiry must not overflow.
    anchor
        .checked_add(file_store::remaining(file.expires_at_ms, time)?.min(budget))
        .ok_or_else(capacity)
}
