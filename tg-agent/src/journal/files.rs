//! Immutable file metadata. Bytes and authority remain outside SQLite.
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub struct RegisteredFile {
    pub(crate) handle: String,
    pub(crate) profile: String,
    pub(crate) principal: String,
    pub(crate) sha256: [u8; 32],
    pub(crate) byte_size: u64,
    pub(crate) filename: String,
    pub(crate) expires_at_ms: i64,
}
impl RegisteredFile {
    pub fn handle(&self) -> &str {
        &self.handle
    }
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
    pub fn byte_size(&self) -> u64 {
        self.byte_size
    }
    pub fn filename(&self) -> &str {
        &self.filename
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}
impl fmt::Debug for RegisteredFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegisteredFile([REDACTED])")
    }
}
impl Journal {
    pub(crate) async fn insert_file_metadata(
        &self,
        file: RegisteredFile,
    ) -> Result<RegisteredFile> {
        self.run(move |connection,options,_| {
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;
            if file.expires_at_ms<=time { return Err(JournalError::new(JournalErrorKind::Expired)); }
            let count:i64=transaction.query_row("SELECT COUNT(*) FROM executor_files",[],|row|row.get(0)).map_err(|_|storage())?;
            if count>=options.max_file_handles as i64 { return Err(JournalError::new(JournalErrorKind::Capacity)); }
            transaction.execute("INSERT INTO executor_files(handle,profile,principal,sha256,byte_size,filename,created_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![file.handle,file.profile,file.principal,file.sha256.as_slice(),file.byte_size as i64,file.filename,time,file.expires_at_ms]).map_err(|error| {
                if error.sqlite_error_code()==Some(rusqlite::ErrorCode::ConstraintViolation) { JournalError::new(JournalErrorKind::Conflict) } else { storage() }
            })?;
            transaction.commit().map_err(|_|storage())?;
            Ok(file)
        }).await
    }
    pub(crate) async fn file_metadata(
        &self,
        profile: &str,
        principal: &str,
        handle: &str,
    ) -> Result<Option<RegisteredFile>> {
        for value in [profile, principal, handle] {
            types::valid_id(value)?;
        }
        let (profile, principal, handle) =
            (profile.to_owned(), principal.to_owned(), handle.to_owned());
        self.run(move |connection,_,_| {
            connection.query_row("SELECT sha256,byte_size,filename,expires_at_ms FROM executor_files WHERE handle=?1 AND profile=?2 AND principal=?3",params![handle,profile,principal],|row| {
                Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?))
            }).optional().map_err(|_|storage())?.map(|(hash,byte_size,filename,expires_at_ms)| {
                Ok(RegisteredFile{handle,profile,principal,sha256:hash.try_into().map_err(|_|storage())?,byte_size:u64::try_from(byte_size).map_err(|_|storage())?,filename,expires_at_ms})
            }).transpose()
        }).await
    }
}
