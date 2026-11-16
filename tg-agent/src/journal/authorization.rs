//! Storage primitives for A1. Authority is checked by the executor, not these rows.
use super::*;
use crate::executor::{Action, Budget, ChatId};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileScope {
    pub handle: String,
    pub sha256: [u8; 32],
    pub bytes: u64,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct OperationScope {
    pub bot_id: i64,
    pub action: Action,
    pub chat: Option<ChatId>,
    pub file: Option<FileScope>,
    pub reference_hash: Option<[u8; 32]>,
}
impl fmt::Debug for OperationScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationScope([REDACTED])")
    }
}
impl OperationScope {
    fn validate(&self) -> Result<()> {
        if self.bot_id < 0
            || !self.action.is_write()
            || (self.action == Action::SendDocument) != self.file.is_some()
            || (self.action == Action::AnswerCallbackQuery) != self.reference_hash.is_some()
            || (self.action != Action::AnswerCallbackQuery && self.chat.is_none())
            || self
                .file
                .as_ref()
                .is_some_and(|v| types::valid_id(&v.handle).is_err() || v.bytes > i64::MAX as u64)
        {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct BudgetContext {
    pub profile: String,
    pub principal: String,
    pub budget: Budget,
    pub cost: u64,
}
#[derive(Default, Clone, Copy)]
pub(super) struct Charge {
    pub invocations: u64,
    pub write_attempts: u64,
    pub upload_bytes: u64,
    pub cost_units: u64,
}
impl Journal {
    /// Immutable metadata lookup, including ownership. Unlike get_scoped this
    /// never refreshes state or materializes a cached result (dry-run/approval).
    pub(crate) async fn operation_scope(
        &self,
        profile: &str,
        principal: &str,
        id: &str,
    ) -> Result<Option<OperationScope>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |connection,_,_| {
            let action: Option<String> = connection.query_row("SELECT action FROM operations WHERE profile=?1 AND operation_id=?2 AND principal=?3",params![profile,id,principal],|row|row.get(0)).optional().map_err(|_|storage())?;
            let Some(action) = action else { return Ok(None); };
            let scope = load_scope(connection,&profile,&id)?.ok_or_else(||JournalError::new(JournalErrorKind::Denied))?;
            if scope.action.name() != action { return Err(storage()); }
            Ok(Some(scope))
        }).await
    }
    pub(crate) async fn release_prepared(&self, prepared: Prepared) -> Result<()> {
        self.run_fenced(move |connection, _, worker| {
            let lease = prepared.0;
            if lease.issuer != worker { return Err(JournalError::new(JournalErrorKind::LeaseLost)); }
            let count = connection.execute("UPDATE operations SET worker=NULL,lease_until_ms=NULL,revision=revision+1,updated_at_ms=?1 WHERE profile=?2 AND operation_id=?3 AND principal=?4 AND worker=?5 AND revision=?6 AND state='prepared'", params![now()?,lease.request.profile,lease.request.id,lease.request.principal,worker,lease.revision]).map_err(|_| storage())?;
            if count != 1 { return Err(JournalError::new(JournalErrorKind::LeaseLost)); }
            Ok(())
        }).await
    }
    pub(crate) async fn prepare_scoped(
        &self,
        request: OperationRequest,
        scope: OperationScope,
    ) -> Result<Preparation> {
        scope.validate()?;
        self.prepare_checked(request, move |connection, request, created| {
            let bot: Option<i64> = connection.query_row("SELECT bot_id FROM update_feeds WHERE profile=?1", [&request.profile], |row| row.get(0)).optional().map_err(|_| storage())?;
            if bot != Some(scope.bot_id) || scope.action.name() != request.action { return Err(JournalError::new(JournalErrorKind::Denied)); }
            if !created {
                let stored = load_scope(connection, &request.profile, &request.id)?.ok_or_else(|| JournalError::new(JournalErrorKind::Denied))?;
                if stored != scope { return Err(JournalError::new(JournalErrorKind::Conflict)); }
                return Ok(());
            }
            let (handle, hash, bytes) = match &scope.file {
                Some(file) => (Some(file.handle.as_str()), Some(file.sha256.as_slice()), Some(file.bytes as i64)),
                None => (None, None, None),
            };
            connection.execute("INSERT INTO executor_operation_scopes(profile,operation_id,bot_id,action,chat_id,file_handle,file_sha256,file_bytes,reference_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![request.profile, request.id, scope.bot_id, scope.action.name(), scope.chat.map(ChatId::get), handle, hash, bytes, scope.reference_hash.as_ref().map(|v|v.as_slice())]).map_err(|_| storage())?;
            Ok(())
        }).await
    }
    #[cfg(test)]
    pub(crate) async fn begin_scoped(
        &self,
        prepared: Prepared,
        context: BudgetContext,
    ) -> Result<InFlight> {
        self.begin_authorized(prepared, move |_| Ok(context)).await
    }
    pub(crate) async fn begin_authorized(
        &self,
        prepared: Prepared,
        authorize: impl FnOnce(i64) -> Result<BudgetContext> + Send + 'static,
    ) -> Result<InFlight> {
        self.begin_checked(prepared, move |connection, options, request, time| {
            let context = authorize(time)?;
            if context.profile != request.profile || context.principal != request.principal { return Err(JournalError::new(JournalErrorKind::Denied)); }
            let scope = load_scope(connection, &request.profile, &request.id)?.ok_or_else(|| JournalError::new(JournalErrorKind::Denied))?;
            if scope.action.name() != request.action { return Err(JournalError::new(JournalErrorKind::Conflict)); }
            // The partial unique index makes all aliases of this callback share
            // one admitted operation, including after Unknown and NotSent.
            connection.execute("UPDATE executor_operation_scopes SET admitted=1 WHERE profile=?1 AND operation_id=?2", params![request.profile, request.id]).map_err(|error| {
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) { JournalError::new(JournalErrorKind::Conflict) } else { storage() }
            })?;
            charge(connection, options, &context, Charge { write_attempts:1, upload_bytes:scope.file.map_or(0, |v|v.bytes), cost_units:context.cost, ..Charge::default() }, time)
        }).await
    }
    #[cfg(test)]
    pub(crate) async fn charge_invocation(&self, context: BudgetContext, cost: u64) -> Result<()> {
        self.charge_authorized_invocation(move |_| Ok((context, cost)))
            .await
    }
    pub(crate) async fn charge_authorized_invocation(
        &self,
        authorize: impl FnOnce(i64) -> Result<(BudgetContext, u64)> + Send + 'static,
    ) -> Result<()> {
        self.run(move |connection, options, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let time = now()?;
            let (context, cost) = authorize(time)?;
            charge(
                &transaction,
                options,
                &context,
                Charge {
                    invocations: 1,
                    cost_units: cost,
                    ..Charge::default()
                },
                time,
            )?;
            transaction.commit().map_err(|_| storage())
        })
        .await
    }
    pub(crate) async fn get_scoped(
        &self,
        profile: &str,
        principal: &str,
        id: &str,
    ) -> Result<Option<(Operation, OperationScope)>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |connection, _, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let Some(mut stored) = load_for(&transaction, &profile, &id, Some(&principal))? else {
                return Ok(None);
            };
            let scope = load_scope(&transaction, &profile, &id)?
                .ok_or_else(|| JournalError::new(JournalErrorKind::Denied))?;
            if scope.action.name() != stored.request.action {
                return Err(storage());
            }
            refresh(&transaction, &mut stored, now()?)?;
            transaction.commit().map_err(|_| storage())?;
            Ok(Some((stored.operation, scope)))
        })
        .await
    }
}
fn load_scope(connection: &Connection, profile: &str, id: &str) -> Result<Option<OperationScope>> {
    let stored = connection.query_row("SELECT bot_id,action,chat_id,file_handle,file_sha256,file_bytes,reference_hash FROM executor_operation_scopes WHERE profile=?1 AND operation_id=?2", params![profile,id], |row| {
        Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<i64>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<Vec<u8>>>(4)?,row.get::<_,Option<i64>>(5)?,row.get::<_,Option<Vec<u8>>>(6)?))
    }).optional().map_err(|_| storage())?;
    stored
        .map(|(bot_id, action, chat, handle, hash, bytes, reference)| {
            let file = match (handle, hash, bytes) {
                (Some(handle), Some(hash), Some(bytes)) => Some(FileScope {
                    handle,
                    sha256: hash.try_into().map_err(|_| storage())?,
                    bytes: u64::try_from(bytes).map_err(|_| storage())?,
                }),
                (None, None, None) => None,
                _ => return Err(storage()),
            };
            let scope = OperationScope {
                bot_id,
                action: Action::ALL
                    .into_iter()
                    .find(|v| v.name() == action)
                    .ok_or_else(storage)?,
                chat: chat.map(ChatId::new).transpose().map_err(|_| storage())?,
                file,
                reference_hash: reference
                    .map(|v| v.try_into().map_err(|_| storage()))
                    .transpose()?,
            };
            scope.validate().map_err(|_| storage())?;
            Ok(scope)
        })
        .transpose()
}
pub(super) fn charge(
    connection: &Connection,
    options: &JournalOptions,
    context: &BudgetContext,
    delta: Charge,
    time: i64,
) -> Result<()> {
    context
        .budget
        .validate()
        .map_err(|_| JournalError::new(JournalErrorKind::Validation))?;
    for value in [
        context.profile.as_str(),
        context.principal.as_str(),
        context.budget.id(),
    ] {
        types::valid_id(value)?;
    }
    if time < 0 {
        return Err(storage());
    }
    let window = types::millis(context.budget.window_duration())?;
    let candidate = time - time % window;
    let existing = connection.query_row("SELECT window_ms,window_start_ms,invocations,write_attempts,upload_bytes,cost_units FROM executor_budgets WHERE profile=?1 AND principal=?2 AND budget_id=?3", params![context.profile,context.principal,context.budget.id()], |row| {
        Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,[row.get::<_,i64>(2)?,row.get(3)?,row.get(4)?,row.get(5)?]))
    }).optional().map_err(|_| storage())?;
    let (start, usage) = match existing {
        Some((period, start, usage)) => {
            if period != window {
                return Err(JournalError::new(JournalErrorKind::Conflict));
            }
            if candidate > start {
                (candidate, [0; 4])
            } else {
                (start, usage)
            }
        }
        None => {
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM executor_budgets", [], |row| {
                    row.get(0)
                })
                .map_err(|_| storage())?;
            if count >= options.max_budget_ledgers as i64 {
                return Err(JournalError::new(JournalErrorKind::Capacity));
            }
            (candidate, [0; 4])
        }
    };
    let limits = context.budget.limit_values();
    let caps = [
        limits.invocations,
        limits.write_attempts,
        limits.upload_bytes,
        limits.cost_units,
    ];
    let changes = [
        delta.invocations,
        delta.write_attempts,
        delta.upload_bytes,
        delta.cost_units,
    ];
    let mut next = [0i64; 4];
    for index in 0..4 {
        next[index] = u64::try_from(usage[index])
            .ok()
            .and_then(|v| v.checked_add(changes[index]))
            .filter(|v| *v <= caps[index])
            .and_then(|v| i64::try_from(v).ok())
            .ok_or_else(|| JournalError::new(JournalErrorKind::QuotaExceeded))?;
    }
    connection.execute("INSERT INTO executor_budgets(profile,principal,budget_id,window_ms,window_start_ms,invocations,write_attempts,upload_bytes,cost_units) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(profile,principal,budget_id) DO UPDATE SET window_start_ms=excluded.window_start_ms,invocations=excluded.invocations,write_attempts=excluded.write_attempts,upload_bytes=excluded.upload_bytes,cost_units=excluded.cost_units", params![context.profile,context.principal,context.budget.id(),window,start,next[0],next[1],next[2],next[3]]).map_err(|_| storage())?;
    Ok(())
}
