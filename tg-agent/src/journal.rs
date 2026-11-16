//! Persistent reservations and receipts. This module never performs a Telegram call.
#[cfg(feature = "executor")]
pub(crate) mod authorization;
#[cfg(all(test, feature = "executor"))]
#[path = "journal/authorization_tests.rs"]
mod authorization_tests;
pub(crate) mod types;
mod worker;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    fmt,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub use types::*;

#[derive(Clone)]
pub struct Journal {
    inner: Arc<worker::Database>,
    #[cfg(feature = "executor")]
    pub(crate) callback_receipts: Arc<crate::updates::ReceiptTracker>,
}
impl fmt::Debug for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Journal").finish_non_exhaustive()
    }
}
impl Journal {
    #[cfg(feature = "executor")]
    pub(crate) fn origin_token(&self) -> Arc<str> {
        self.inner.origin_token()
    }
    /// The executor must keep its network deadline and receipt margin within this lease.
    pub fn lease_duration(&self) -> Duration {
        self.inner.options.lease
    }

    /// Use an absolute local path in a directory protected by the host's OS permissions.
    pub async fn open(path: impl AsRef<Path>, options: JournalOptions) -> Result<Self> {
        options.validate()?;
        let path = path.as_ref().to_owned();
        if !path.is_absolute() {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        #[cfg(feature = "executor")]
        let receipts = Arc::new(crate::updates::ReceiptTracker::new(
            options.max_update_entries.min(65536),
        ));
        Ok(Self {
            inner: Arc::new(worker::Database::open(path, options).await?),
            #[cfg(feature = "executor")]
            callback_receipts: receipts,
        })
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection, &JournalOptions, &str) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.inner.call(action).await
    }

    /// Maintenance/confirmation/release wait fairly for a bounded slot instead of Capacity.
    pub(crate) async fn run_fenced<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection, &JournalOptions, &str) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.inner.call_fenced(action).await
    }

    /// Stop admission on every clone, drain database jobs and await connection close.
    /// The host must drain external Telegram actions before closing their journal.
    pub async fn close(&self) -> Result<()> {
        self.inner.close().await
    }

    pub async fn prepare(&self, request: OperationRequest) -> Result<Preparation> {
        self.prepare_checked(request, |_, _, _| Ok(())).await
    }

    pub(crate) async fn prepare_checked(
        &self,
        request: OperationRequest,
        bind: impl FnOnce(&Connection, &OperationRequest, bool) -> Result<()> + Send + 'static,
    ) -> Result<Preparation> {
        self.run(move |connection, options, worker| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage())?;
            let time = now()?;
            let lease_until = deadline(time, options.lease)?;
            let revision;
            if let Some(mut stored) = load_for(&transaction, &request.profile, &request.id, Some(&request.principal))? {
                if stored.request.principal != request.principal { return Err(JournalError::new(JournalErrorKind::Denied)); }
                if stored.request.action != request.action || stored.request.hash != request.hash { return Err(JournalError::new(JournalErrorKind::Conflict)); }
                refresh(&transaction, &mut stored, time)?;
                if stored.operation.expired || stored.operation.state != OperationState::Prepared || stored.lease_until.is_some_and(|expires| expires > time) {
                    bind(&transaction, &request, false)?;
                    transaction.commit().map_err(|_| storage())?;
                    return Ok(Preparation::Existing(stored.operation));
                }
                revision = stored.operation.revision.checked_add(1).ok_or_else(storage)?;
                transaction.execute("UPDATE operations SET worker=?1,lease_until_ms=?2,revision=?3,updated_at_ms=?4 WHERE profile=?5 AND operation_id=?6", params![worker, lease_until.min(stored.operation.expires_at_ms), revision, time, request.profile, request.id]).map_err(|_| storage())?;
                bind(&transaction, &request, false)?;
            } else {
                let exists: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE profile=?1 AND operation_id=?2)",params![request.profile,request.id],|row|row.get(0)).map_err(|_|storage())?;
                if exists { return Err(JournalError::new(JournalErrorKind::Denied)); }
                // ponytail: exact COUNT under the write transaction; a stored count can replace it if large journals need it.
                let count: i64 = transaction.query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0)).map_err(|_| storage())?;
                if count >= options.max_operations as i64 { return Err(JournalError::new(JournalErrorKind::Capacity)); }
                let expires = deadline(time, options.retention)?;
                revision = 1;
                transaction.execute("INSERT INTO operations(profile,operation_id,principal,action,payload_hash,state,revision,worker,lease_until_ms,created_at_ms,updated_at_ms,expires_at_ms) VALUES (?1,?2,?3,?4,?5,'prepared',1,?6,?7,?8,?8,?9)", params![request.profile,request.id,request.principal,request.action,request.hash.as_slice(),worker,lease_until.min(expires),time,expires]).map_err(|_| storage())?;
                bind(&transaction, &request, true)?;
            }
            transaction.commit().map_err(|_| storage())?;
            Ok(Preparation::Ready(Prepared(types::Lease { request, revision, issuer:worker.into() })))
        }).await
    }

    /// Commit this transition before polling any future that can perform a Telegram action.
    pub async fn begin(&self, prepared: Prepared) -> Result<InFlight> {
        self.begin_checked(prepared, |_, _, _, _| Ok(())).await
    }

    pub(crate) async fn begin_checked(
        &self,
        prepared: Prepared,
        admit: impl FnOnce(&Connection, &JournalOptions, &OperationRequest, i64) -> Result<()>
        + Send
        + 'static,
    ) -> Result<InFlight> {
        self.run(move |connection, options, worker| {
            let lease = prepared.0;
            if lease.issuer!=worker{return Err(JournalError::new(JournalErrorKind::LeaseLost));}
            let revision = lease.revision.checked_add(1).ok_or_else(storage)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage())?;
            let time = now()?;
            let count = transaction.execute("UPDATE operations SET state='in_flight',failure_json=NULL,revision=?1,lease_until_ms=min(?2,expires_at_ms),updated_at_ms=?3 WHERE profile=?4 AND operation_id=?5 AND principal=?6 AND worker=?7 AND revision=?8 AND state='prepared' AND lease_until_ms>?3 AND expires_at_ms>?3 AND expired=0", params![revision,deadline(time,options.lease)?,time,lease.request.profile,lease.request.id,lease.request.principal,worker,lease.revision]).map_err(|_| storage())?;
            if count != 1 { return Err(JournalError::new(JournalErrorKind::LeaseLost)); }
            admit(&transaction, options, &lease.request, time)?;
            let until = transaction.query_row("SELECT lease_until_ms FROM operations WHERE profile=?1 AND operation_id=?2", params![lease.request.profile,lease.request.id], |row| row.get(0)).map_err(|_| storage())?;
            transaction.commit().map_err(|_| storage())?;
            Ok(InFlight(types::Lease { request: lease.request, revision,issuer:lease.issuer }, until))
        }).await
    }

    pub async fn complete(&self, flight: InFlight, completion: Completion) -> Result<Operation> {
        self.complete_with_admission(flight, completion, false)
            .await
    }
    #[cfg(feature = "executor")]
    pub(crate) async fn complete_fenced(
        &self,
        flight: InFlight,
        completion: Completion,
    ) -> Result<Operation> {
        self.complete_with_admission(flight, completion, true).await
    }
    async fn complete_with_admission(
        &self,
        flight: InFlight,
        completion: Completion,
        wait: bool,
    ) -> Result<Operation> {
        let action = move |connection: &mut Connection, options: &JournalOptions, worker: &str| {
            let lease = flight.0;
            if lease.issuer != worker {
                return Err(JournalError::new(JournalErrorKind::LeaseLost));
            }
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let (result, availability) = match completion.result {
                Some(value) if options.retain_results => {
                    match types::encode(&value, options.max_result_bytes) {
                        Ok(bytes) => (
                            Some(String::from_utf8(bytes).map_err(|_| storage())?),
                            ResultAvailability::Stored,
                        ),
                        Err(_) => (None, ResultAvailability::TooLarge),
                    }
                }
                Some(_) => (None, ResultAvailability::Disabled),
                None => (None, ResultAvailability::NotApplicable),
            };
            let failure = completion
                .failure
                .map(|value| serde_json::to_string(&value))
                .transpose()
                .map_err(|_| storage())?;
            let count = transaction.execute("UPDATE operations SET state=?1,revision=revision+1,worker=NULL,lease_until_ms=NULL,updated_at_ms=?2,result_json=?3,result_availability=?4,failure_json=?5 WHERE profile=?6 AND operation_id=?7 AND principal=?8 AND worker=?9 AND revision=?10 AND state='in_flight'", params![completion.state.as_str(),now()?,result,availability.as_str(),failure,lease.request.profile,lease.request.id,lease.request.principal,worker,lease.revision]).map_err(|_| storage())?;
            if count != 1 {
                return Err(JournalError::new(JournalErrorKind::LeaseLost));
            }
            let mut stored = load(&transaction, &lease.request.profile, &lease.request.id)?
                .ok_or_else(storage)?;
            refresh(&transaction, &mut stored, now()?)?;
            transaction.commit().map_err(|_| storage())?;
            Ok(stored.operation)
        };
        if wait {
            self.run_fenced(action).await
        } else {
            self.run(action).await
        }
    }

    /// The host must also check current grants before disclosing this snapshot.
    pub async fn get(&self, profile: &str, principal: &str, id: &str) -> Result<Option<Operation>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |connection, _, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let mut stored = load_for(&transaction, &profile, &id, Some(&principal))?;
            if let Some(stored) = &mut stored {
                refresh(&transaction, stored, now()?)?;
            }
            transaction.commit().map_err(|_| storage())?;
            Ok(stored.map(|stored| stored.operation))
        })
        .await
    }

    /// Fence expired in-flight attempts as Unknown. Active leases are never reset on startup.
    pub async fn recover_expired(&self, limit: usize) -> Result<usize> {
        self.maintain(limit, true).await
    }
    /// Expire result bodies and unstarted leases, retaining every operation key.
    pub async fn prune_expired(&self, limit: usize) -> Result<usize> {
        self.maintain(limit, false).await
    }
    async fn maintain(&self, limit: usize, recovery: bool) -> Result<usize> {
        if !(1..=1000).contains(&limit) {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        self.run(move |connection,_,_| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time = now()?;
            let sql = if recovery {
                "SELECT profile,operation_id FROM operations WHERE state='in_flight' AND lease_until_ms<=?1 ORDER BY lease_until_ms,profile,operation_id LIMIT ?2"
            } else {
                "SELECT profile,operation_id FROM operations WHERE expired=0 AND expires_at_ms<=?1 ORDER BY expires_at_ms,profile,operation_id LIMIT ?2"
            };
            let keys = {
                let mut statement = transaction.prepare(sql).map_err(|_|storage())?;
                statement.query_map(params![time,limit as i64],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))
                    .map_err(|_|storage())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|storage())?
            };
            let mut count = 0;
            for (profile,id) in keys {
                let mut stored = load(&transaction,&profile,&id)?.ok_or_else(storage)?;
                count += usize::from(refresh(&transaction,&mut stored,time)?);
            }
            transaction.commit().map_err(|_|storage())?;
            Ok(count)
        }).await
    }
}

pub(crate) fn storage() -> JournalError {
    JournalError::new(JournalErrorKind::Storage)
}
pub(crate) fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or_else(storage)
}
pub(crate) fn deadline(time: i64, duration: Duration) -> Result<i64> {
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
    } else if application != APPLICATION || !(1..=4).contains(&version) {
        return Err(JournalError::new(JournalErrorKind::UnsupportedSchema));
    }
    if version < 2 {
        verify_schema(&transaction, &[include_str!("journal/schema.sql")])?;
        transaction
            .execute_batch(include_str!("updates/schema.sql"))
            .map_err(|_| storage())?;
        transaction
            .pragma_update(None, "user_version", 2)
            .map_err(|_| storage())?;
    }
    if version < 3 {
        verify_schema(
            &transaction,
            &[
                include_str!("journal/schema.sql"),
                include_str!("updates/schema.sql"),
            ],
        )?;
        transaction
            .execute_batch(include_str!("updates/consumers.sql"))
            .map_err(|_| storage())?;
        transaction
            .pragma_update(None, "user_version", 3)
            .map_err(|_| storage())?;
    }
    if version < 4 {
        verify_schema(
            &transaction,
            &[
                include_str!("journal/schema.sql"),
                include_str!("updates/schema.sql"),
                include_str!("updates/consumers.sql"),
            ],
        )?;
        transaction
            .execute_batch(include_str!("journal/authorization.sql"))
            .map_err(|_| storage())?;
        transaction
            .pragma_update(None, "user_version", 4)
            .map_err(|_| storage())?;
    }
    verify_schema(
        &transaction,
        &[
            include_str!("journal/schema.sql"),
            include_str!("updates/schema.sql"),
            include_str!("updates/consumers.sql"),
            include_str!("journal/authorization.sql"),
        ],
    )?;
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
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| storage())?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|_| storage())?;
    let foreign_keys: bool = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(|_| storage())?;
    if !mode.eq_ignore_ascii_case("wal") || synchronous != 2 || !foreign_keys {
        return Err(storage());
    }
    Ok(())
}

fn verify_schema(connection: &Connection, schemas: &[&str]) -> Result<()> {
    // The reviewed DDL is part of this version, including constraints and indexes.
    let mut expected_count = 0;
    for expected in schemas
        .iter()
        .flat_map(|schema| schema.split(';'))
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
    {
        let name_index = if expected.starts_with("CREATE UNIQUE INDEX ") {
            3
        } else {
            2
        };
        let name = expected
            .split_whitespace()
            .nth(name_index)
            .ok_or_else(storage)?;
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage())?;
        if actual.as_deref().map(str::trim) != Some(expected) {
            return Err(JournalError::new(JournalErrorKind::UnsupportedSchema));
        }
        expected_count += 1;
    }
    let actual_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| storage())?;
    if actual_count != expected_count {
        return Err(JournalError::new(JournalErrorKind::UnsupportedSchema));
    }
    Ok(())
}

struct Stored {
    request: OperationRequest,
    operation: Operation,
    lease_until: Option<i64>,
}
fn load(connection: &Connection, profile: &str, id: &str) -> Result<Option<Stored>> {
    load_for(connection, profile, id, None)
}
fn load_for(
    connection: &Connection,
    profile: &str,
    id: &str,
    principal: Option<&str>,
) -> Result<Option<Stored>> {
    let raw = connection.query_row("SELECT principal,action,payload_hash,state,revision,created_at_ms,updated_at_ms,expires_at_ms,expired,result_json,result_availability,failure_json,lease_until_ms FROM operations WHERE profile=?1 AND operation_id=?2 AND (?3 IS NULL OR principal=?3)",params![profile,id,principal],|row| {
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

fn refresh(connection: &Connection, stored: &mut Stored, time: i64) -> Result<bool> {
    let operation = &mut stored.operation;
    let expired = operation.expires_at_ms <= time;
    let recover = operation.state == OperationState::InFlight
        && (expired || stored.lease_until.is_none_or(|expires| expires <= time));
    let prune = expired && !operation.expired;
    if !recover && !prune {
        return Ok(false);
    }
    if recover {
        operation.state = OperationState::Unknown;
        operation.failure = Some(AttemptFailure {
            kind: "lease_expired".into(),
            outcome: "unknown".into(),
            http_status: None,
            api_error_code: None,
            retry_after_secs: None,
            migrate_to_chat_id: None,
        });
    }
    if prune {
        operation.expired = true;
        operation.result = None;
        if operation.result_availability == ResultAvailability::Stored {
            operation.result_availability = ResultAvailability::Expired;
        }
    }
    operation.revision = operation.revision.checked_add(1).ok_or_else(storage)?;
    operation.updated_at_ms = time;
    let failure = operation
        .failure
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| storage())?;
    let count = connection.execute("UPDATE operations SET state=?1,revision=?2,worker=NULL,lease_until_ms=NULL,updated_at_ms=?3,expired=?4,result_json=NULL,result_availability=?5,failure_json=?6 WHERE profile=?7 AND operation_id=?8 AND revision=?9",params![operation.state.as_str(),operation.revision,time,operation.expired,operation.result_availability.as_str(),failure,stored.request.profile,stored.request.id,operation.revision-1]).map_err(|_|storage())?;
    if count != 1 {
        return Err(JournalError::new(JournalErrorKind::LeaseLost));
    }
    stored.lease_until = None;
    Ok(true)
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

    #[tokio::test]
    async fn cancelled_database_work_keeps_its_capacity_permit_until_it_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("operations.sqlite"),
            JournalOptions::default().max_pending(1),
        )
        .await
        .unwrap();
        let request =
            OperationRequest::new("bot", "alice", "one", "send_message", &json!({})).unwrap();
        journal.prepare(request).await.unwrap();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = journal.clone();
        let task = tokio::spawn(async move {
            worker.run(move |connection,_,_| {
            let _ = started.send(());
            blocked.recv_timeout(Duration::from_secs(5)).map_err(|_|storage())?;
            connection.execute("UPDATE operations SET expired=1,worker=NULL,lease_until_ms=NULL,revision=revision+1 WHERE operation_id='one'",[]).map_err(|_|storage())?;
            Ok(())
        }).await
        });
        waiting.await.unwrap();
        task.abort();
        assert_eq!(
            journal.get("bot", "alice", "one").await.unwrap_err().kind(),
            JournalErrorKind::Capacity
        );
        let mut closing = Box::pin(journal.close());
        tokio::select! {
            biased;
            result = &mut closing => panic!("must drain admitted work: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        assert_eq!(
            journal.get("bot", "alice", "one").await.unwrap_err().kind(),
            JournalErrorKind::Closed
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), closing)
            .await
            .unwrap()
            .unwrap();
        let reopened = Journal::open(
            directory.path().join("operations.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        assert!(
            reopened
                .get("bot", "alice", "one")
                .await
                .unwrap()
                .unwrap()
                .expired()
        );
        reopened.close().await.unwrap();
        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn begin_rechecks_lease_and_lifetime_after_waiting_for_the_write_lock() {
        for lifetime in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("operations.sqlite");
            let options = if lifetime {
                JournalOptions::default().retention(Duration::from_millis(500))
            } else {
                JournalOptions::default().lease_duration(Duration::from_millis(500))
            };
            let journal = Journal::open(&path, options).await.unwrap();
            let request =
                OperationRequest::new("bot", "alice", "one", "send_message", &json!({})).unwrap();
            let Preparation::Ready(lease) = journal.prepare(request).await.unwrap() else {
                panic!("expected reservation");
            };
            let (entered, waiting) = tokio::sync::oneshot::channel();
            journal
                .run(move |connection, _, _| {
                    let mut entered = Some(entered);
                    connection
                        .busy_handler(Some(move |attempt| {
                            if let Some(entered) = entered.take() {
                                let _ = entered.send(());
                            }
                            std::thread::sleep(Duration::from_millis(1));
                            attempt < 2000
                        }))
                        .map_err(|_| storage())?;
                    Ok(())
                })
                .await
                .unwrap();
            let mut blocker = Connection::open(&path).unwrap();
            let transaction = blocker
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let expires: i64 = transaction
                .query_row(
                    "SELECT lease_until_ms FROM operations WHERE operation_id='one'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let worker = journal.clone();
            let task = tokio::spawn(async move { worker.begin(lease).await });
            tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap();
            let remaining = expires - now().unwrap();
            assert!(
                remaining > 0,
                "fixture must enter lock wait before lease expiry"
            );
            tokio::time::sleep(Duration::from_millis(remaining as u64 + 20)).await;
            transaction.commit().unwrap();
            assert_eq!(
                task.await.unwrap().unwrap_err().kind(),
                JournalErrorKind::LeaseLost
            );
        }
    }

    #[tokio::test]
    async fn the_sqlite_connection_is_closed_on_its_database_thread_when_last_handle_drops() {
        struct ObserveDrop(Option<tokio::sync::oneshot::Sender<std::thread::ThreadId>>);
        impl Drop for ObserveDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(std::thread::current().id());
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("operations.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let (closed, waiting) = tokio::sync::oneshot::channel();
        let database_thread = journal
            .run(move |connection, _, _| {
                let observer = ObserveDrop(Some(closed));
                connection
                    .busy_handler(Some(move |_| {
                        std::hint::black_box(&observer);
                        false
                    }))
                    .map_err(|_| storage())?;
                Ok(std::thread::current().id())
            })
            .await
            .unwrap();
        assert_ne!(database_thread, std::thread::current().id());
        drop(journal);
        let closed_on = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            closed_on, database_thread,
            "closing SQLite must not perform checkpoint I/O on the async runtime"
        );
    }
    #[tokio::test]
    async fn ack_and_release_wait_behind_a_cancelled_admitted_job() {
        use crate::updates::{SourceMode, UpdateFeed, UpdateOptions};
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("updates.sqlite"),
            JournalOptions::default().max_pending(1),
        )
        .await
        .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        let source = feed.claim_source(SourceMode::Polling).await.unwrap();
        let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
        feed.append_poll(
            &source,
            &checkpoint,
            vec![serde_json::from_value(serde_json::json!({"update_id":1})).unwrap()],
        )
        .await
        .unwrap();
        let consumer = feed.claim_consumer("router").await.unwrap();
        let (_, ack) = feed
            .read_consumer(&consumer, 100)
            .await
            .unwrap()
            .into_parts();
        for mode in [0, 1, 2] {
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (unblock, blocked) = std::sync::mpsc::channel();
            let worker = journal.clone();
            let task = tokio::spawn(async move {
                worker
                    .run(move |_, _, _| {
                        let _ = entered.send(());
                        blocked
                            .recv_timeout(Duration::from_secs(3))
                            .map_err(|_| crate::journal::storage())?;
                        Ok(())
                    })
                    .await
            });
            waiting.await.unwrap();
            task.abort();
            let source = source.clone();
            let consumer = consumer.clone();
            let ack = ack.clone();
            let release = async {
                match mode {
                    0 => feed.release_source(source).await,
                    1 => feed.acknowledge_consumer(&consumer, ack).await.map(|_| ()),
                    _ => feed.release_consumer(consumer).await,
                }
            };
            tokio::pin!(release);
            tokio::select! {biased; result=&mut release=>panic!("cleanup must wait for admitted work: {result:?}"), _=std::future::ready(())=>{} }
            unblock.send(()).unwrap();
            release.await.unwrap();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        feed.claim_source(SourceMode::Polling).await.unwrap();
        let consumer = feed.claim_consumer("router").await.unwrap();
        assert!(
            feed.read_consumer(&consumer, 100)
                .await
                .unwrap()
                .events()
                .is_empty()
        );
        journal.close().await.unwrap();
    }

    #[tokio::test]
    async fn close_wakes_confirmation_waiters_and_releases_ownership_after_draining() {
        use crate::updates::{SourceMode, UpdateFeed, UpdateOptions};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.sqlite");
        let journal = Journal::open(&path, JournalOptions::default().max_pending(1))
            .await
            .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        let source = feed.claim_source(SourceMode::Polling).await.unwrap();
        feed.claim_consumer("router").await.unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (unblock, blocked) = std::sync::mpsc::channel();
        let worker = journal.clone();
        let task = tokio::spawn(async move {
            worker
                .run(move |_, _, _| {
                    let _ = entered.send(());
                    blocked
                        .recv_timeout(Duration::from_secs(3))
                        .map_err(|_| storage())?;
                    Ok(())
                })
                .await
        });
        waiting.await.unwrap();
        task.abort();
        let release = feed.release_source(source);
        tokio::pin!(release);
        tokio::select! {biased; result=&mut release=>panic!("must wait: {result:?}"), _=std::future::ready(())=>{} }
        let closing = journal.close();
        tokio::pin!(closing);
        tokio::select! {biased; result=&mut closing=>panic!("must drain: {result:?}"), _=std::future::ready(())=>{} }
        assert_eq!(release.await.unwrap_err().kind(), JournalErrorKind::Closed);
        unblock.send(()).unwrap();
        closing.await.unwrap();
        assert!(task.await.unwrap_err().is_cancelled());
        let reopened = Journal::open(&path, JournalOptions::default())
            .await
            .unwrap();
        let feed = UpdateFeed::open(reopened.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        feed.claim_source(SourceMode::Polling).await.unwrap();
        feed.claim_consumer("router").await.unwrap();
        reopened.close().await.unwrap();
    }
}
