//! Persistent reservations and receipts. This module never performs a Telegram call.
mod types;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    fmt,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
pub use types::*;

#[derive(Clone)]
pub struct Journal {
    inner: Arc<Inner>,
}
struct Inner {
    connection: std::sync::Mutex<Connection>,
    options: JournalOptions,
    worker: String,
    pending: Arc<Semaphore>,
}
impl fmt::Debug for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Journal").finish_non_exhaustive()
    }
}
impl Journal {
    /// Use an absolute local path in a directory protected by the host's OS permissions.
    pub async fn open(path: impl AsRef<Path>, options: JournalOptions) -> Result<Self> {
        options.validate()?;
        let path = path.as_ref().to_owned();
        if !path.is_absolute() {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        tokio::task::spawn_blocking(move || {
            deadline(now()?, options.lease)?;
            deadline(now()?, options.retention)?;
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|_| storage())?;
            let worker: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            secure_file(&path)?;
            let mut connection = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| storage())?;
            connection
                .busy_timeout(Duration::from_secs(2))
                .map_err(|_| storage())?;
            initialize(&mut connection)?;
            Ok(Self {
                inner: Arc::new(Inner {
                    connection: std::sync::Mutex::new(connection),
                    pending: Arc::new(Semaphore::new(options.max_pending)),
                    options,
                    worker,
                }),
            })
        })
        .await
        .map_err(|_| storage())?
    }

    async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection, &JournalOptions, &str) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .inner
            .pending
            .clone()
            .try_acquire_owned()
            .map_err(|_| JournalError::new(JournalErrorKind::Capacity))?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // ponytail: one connection per instance; bounded jobs wait here, add a pool only after profiling.
            let mut connection = inner.connection.lock().map_err(|_| storage())?;
            action(&mut connection, &inner.options, &inner.worker)
        })
        .await
        .map_err(|_| storage())?
    }

    pub async fn prepare(&self, request: OperationRequest) -> Result<Preparation> {
        self.run(move |connection, options, worker| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage())?;
            let time = now()?;
            let lease_until = deadline(time, options.lease)?;
            let revision;
            if let Some(stored) = load(&transaction, &request.profile, &request.id)? {
                if stored.request.principal != request.principal { return Err(JournalError::new(JournalErrorKind::Denied)); }
                if stored.request.action != request.action || stored.request.hash != request.hash { return Err(JournalError::new(JournalErrorKind::Conflict)); }
                if stored.operation.expired || stored.operation.expires_at_ms <= time { return Ok(Preparation::Existing(stored.operation)); }
                if stored.operation.state != OperationState::Prepared || stored.lease_until.is_some_and(|expires| expires > time) { return Ok(Preparation::Existing(stored.operation)); }
                revision = stored.operation.revision.checked_add(1).ok_or_else(storage)?;
                transaction.execute("UPDATE operations SET worker=?1,lease_until_ms=?2,revision=?3,updated_at_ms=?4 WHERE profile=?5 AND operation_id=?6", params![worker, lease_until.min(stored.operation.expires_at_ms), revision, time, request.profile, request.id]).map_err(|_| storage())?;
            } else {
                // ponytail: exact COUNT under the write transaction; a stored count can replace it if large journals need it.
                let count: i64 = transaction.query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0)).map_err(|_| storage())?;
                if count >= options.max_operations as i64 { return Err(JournalError::new(JournalErrorKind::Capacity)); }
                let expires = deadline(time, options.retention)?;
                revision = 1;
                transaction.execute("INSERT INTO operations(profile,operation_id,principal,action,payload_hash,state,revision,worker,lease_until_ms,created_at_ms,updated_at_ms,expires_at_ms) VALUES (?1,?2,?3,?4,?5,'prepared',1,?6,?7,?8,?8,?9)", params![request.profile,request.id,request.principal,request.action,request.hash.as_slice(),worker,lease_until.min(expires),time,expires]).map_err(|_| storage())?;
            }
            transaction.commit().map_err(|_| storage())?;
            Ok(Preparation::Ready(Prepared(types::Lease { request, revision })))
        }).await
    }

    /// Commit this transition before polling any future that can perform a Telegram action.
    pub async fn begin(&self, prepared: Prepared) -> Result<InFlight> {
        self.run(move |connection, options, worker| {
            let lease = prepared.0;
            let time = now()?;
            let revision = lease.revision.checked_add(1).ok_or_else(storage)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage())?;
            let count = transaction.execute("UPDATE operations SET state='in_flight',revision=?1,lease_until_ms=min(?2,expires_at_ms),updated_at_ms=?3 WHERE profile=?4 AND operation_id=?5 AND principal=?6 AND worker=?7 AND revision=?8 AND state='prepared' AND lease_until_ms>?3 AND expires_at_ms>?3 AND expired=0", params![revision,deadline(time,options.lease)?,time,lease.request.profile,lease.request.id,lease.request.principal,worker,lease.revision]).map_err(|_| storage())?;
            if count != 1 { return Err(JournalError::new(JournalErrorKind::LeaseLost)); }
            transaction.commit().map_err(|_| storage())?;
            Ok(InFlight(types::Lease { request: lease.request, revision }))
        }).await
    }

    pub async fn complete(&self, flight: InFlight, completion: Completion) -> Result<Operation> {
        self.run(move |connection, options, worker| {
            let lease = flight.0;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage())?;
            let (result, availability) = match completion.result {
                Some(value) if options.retain_results => match types::encode(&value,options.max_result_bytes) {
                    Ok(bytes) => (Some(String::from_utf8(bytes).map_err(|_| storage())?), ResultAvailability::Stored),
                    Err(_) => (None,ResultAvailability::TooLarge),
                },
                Some(_) => (None,ResultAvailability::Disabled),
                None => (None,ResultAvailability::NotApplicable),
            };
            let failure = completion.failure.map(|value| serde_json::to_string(&value)).transpose().map_err(|_| storage())?;
            let count = transaction.execute("UPDATE operations SET state=?1,revision=revision+1,worker=NULL,lease_until_ms=NULL,updated_at_ms=?2,result_json=?3,result_availability=?4,failure_json=?5 WHERE profile=?6 AND operation_id=?7 AND principal=?8 AND worker=?9 AND revision=?10 AND state='in_flight'", params![completion.state.as_str(),now()?,result,availability.as_str(),failure,lease.request.profile,lease.request.id,lease.request.principal,worker,lease.revision]).map_err(|_| storage())?;
            if count != 1 { return Err(JournalError::new(JournalErrorKind::LeaseLost)); }
            let operation = load(&transaction,&lease.request.profile,&lease.request.id)?.ok_or_else(storage)?.operation;
            transaction.commit().map_err(|_| storage())?;
            Ok(operation)
        }).await
    }

    /// The host must also check current grants before disclosing this snapshot.
    pub async fn get(&self, profile: &str, principal: &str, id: &str) -> Result<Option<Operation>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |connection, _, _| {
            Ok(load(connection, &profile, &id)?
                .filter(|stored| stored.request.principal == principal)
                .map(|stored| stored.operation))
        })
        .await
    }
}

fn storage() -> JournalError {
    JournalError::new(JournalErrorKind::Storage)
}
fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(storage)
}
fn deadline(time: i64, duration: Duration) -> Result<i64> {
    time.checked_add(types::millis(duration)?)
        .ok_or_else(|| JournalError::new(JournalErrorKind::Validation))
}

fn secure_file(path: &Path) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(storage()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)
            .map_err(|_| storage())?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
    }
    Ok(())
}

fn initialize(connection: &mut Connection) -> Result<()> {
    const APPLICATION: i64 = 0x54474131;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| storage())?;
    let application: i64 = transaction
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| storage())?;
    let version: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| storage())?;
    if application == 0 && version == 0 {
        let tables: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| storage())?;
        if tables != 0 {
            return Err(JournalError::new(JournalErrorKind::UnsupportedSchema));
        }
        transaction
            .execute_batch(include_str!("journal/schema.sql"))
            .map_err(|_| storage())?;
        transaction
            .pragma_update(None, "application_id", APPLICATION)
            .map_err(|_| storage())?;
        transaction
            .pragma_update(None, "user_version", 1)
            .map_err(|_| storage())?;
    } else if application != APPLICATION || version != 1 {
        return Err(JournalError::new(JournalErrorKind::UnsupportedSchema));
    }
    transaction.commit().map_err(|_| storage())?;
    let mode: String = connection
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .map_err(|_| storage())?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|_| storage())?;
    connection
        .pragma_update(None, "secure_delete", "ON")
        .map_err(|_| storage())?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|_| storage())?;
    if !mode.eq_ignore_ascii_case("wal") || synchronous != 2 {
        return Err(storage());
    }
    Ok(())
}

struct Stored {
    request: OperationRequest,
    operation: Operation,
    lease_until: Option<i64>,
}
fn load(connection: &Connection, profile: &str, id: &str) -> Result<Option<Stored>> {
    let raw = connection.query_row("SELECT principal,action,payload_hash,state,revision,created_at_ms,updated_at_ms,expires_at_ms,expired,result_json,result_availability,failure_json,lease_until_ms FROM operations WHERE profile=?1 AND operation_id=?2",params![profile,id],|row| {
        Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,String>(3)?,row.get::<_,i64>(4)?,row.get::<_,i64>(5)?,row.get::<_,i64>(6)?,row.get::<_,i64>(7)?,row.get::<_,bool>(8)?,row.get::<_,Option<String>>(9)?,row.get::<_,String>(10)?,row.get::<_,Option<String>>(11)?,row.get::<_,Option<i64>>(12)?))
    }).optional().map_err(|_| storage())?;
    raw.map(
        |(
            principal,
            action,
            hash,
            state,
            revision,
            created_at_ms,
            updated_at_ms,
            expires_at_ms,
            expired,
            result,
            result_availability,
            failure,
            lease_until,
        )| {
            Ok(Stored {
                request: OperationRequest {
                    profile: profile.into(),
                    principal,
                    id: id.into(),
                    action: action.clone(),
                    hash: hash.try_into().map_err(|_| storage())?,
                },
                operation: Operation {
                    profile: profile.into(),
                    operation_id: id.into(),
                    action,
                    state: OperationState::parse(&state)?,
                    revision,
                    created_at_ms,
                    updated_at_ms,
                    expires_at_ms,
                    expired,
                    result_availability: ResultAvailability::parse(&result_availability)?,
                    result: result
                        .map(|value| serde_json::from_str(&value))
                        .transpose()
                        .map_err(|_| storage())?,
                    failure: failure
                        .map(|value| serde_json::from_str(&value))
                        .transpose()
                        .map_err(|_| storage())?,
                },
                lease_until,
            })
        },
    )
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn canonical_json_preserves_arrays_and_rejects_unbounded_inputs() {
        let a = types::encode(&json!({"b":{"z":2,"a":1},"a":[1,2]}), 65536).unwrap();
        assert_eq!(a, br#"{"a":[1,2],"b":{"a":1,"z":2}}"#);
        assert_ne!(
            a,
            types::encode(&json!({"b":{"z":2,"a":1},"a":[2,1]}), 65536).unwrap()
        );
        assert!(types::encode(&json!("x".repeat(70000)), 65536).is_err());
        assert!(types::encode(&json!(vec![0; 8192]), 65536).is_err());
    }
}
