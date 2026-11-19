//! Exact MTProto metadata; rows are not current policy or execution permits.
use super::*;
use crate::{
    journal::authorization::{BudgetContext, Charge, FileScope, charge},
    mtproto::{Action, OriginalScope, PeerId, PeerKind},
    profiles::{ProfileIdentity, ProfileKind},
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    pub account: ProfileIdentity,
    pub action: Action,
    pub peer: Option<PeerId>,
    pub file: Option<FileScope>,
    pub reference_hash: Option<[u8; 32]>,
}
impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MtprotoOperationScope([REDACTED])")
    }
}
impl Scope {
    pub fn policy_scope(&self) -> OriginalScope {
        OriginalScope {
            account: self.account,
            action: self.action,
            peer: self.peer,
            file_handle: self.file.as_ref().map(|f| f.handle.clone()),
        }
    }
    fn validate(&self) -> Result<()> {
        if !matches!(
            self.account.kind(),
            ProfileKind::MtprotoBot | ProfileKind::MtprotoUser
        ) || !self.action.is_write()
            || (self.action == Action::SendDocument) != self.file.is_some()
            || (self.action == Action::AnswerCallbackQuery) != self.reference_hash.is_some()
            || (self.action == Action::AnswerCallbackQuery
                && self.account.kind() != ProfileKind::MtprotoBot)
            || (self.peer.is_none() && self.action != Action::AnswerCallbackQuery)
            || self
                .file
                .as_ref()
                .is_some_and(|f| types::valid_id(&f.handle).is_err() || f.bytes > i64::MAX as u64)
        {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        Ok(())
    }
}
fn account_code(account: ProfileIdentity) -> &'static str {
    match account.kind() {
        ProfileKind::MtprotoBot => "mtproto_bot",
        ProfileKind::MtprotoUser => "mtproto_user",
        _ => "invalid",
    }
}
fn peer_code(peer: PeerId) -> &'static str {
    match peer.kind() {
        PeerKind::User => "user",
        PeerKind::Chat => "chat",
        PeerKind::Channel => "channel",
    }
}
fn denied() -> JournalError {
    JournalError::new(JournalErrorKind::Denied)
}
fn conflict() -> JournalError {
    JournalError::new(JournalErrorKind::Conflict)
}

impl Journal {
    pub(crate) async fn prepare_mtproto(
        &self,
        request: OperationRequest,
        scope: Scope,
    ) -> Result<Preparation> {
        scope.validate()?;
        self.prepare_checked(request,move |c,request,created| {
            if crate::profiles::load(c,&request.profile)?!=Some(scope.account)||request.action!=scope.action.name() {return Err(denied());}
            if let Some(hash)=scope.reference_hash {
                let claimed:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM mtproto_operation_scopes WHERE profile=?1 AND reference_hash=?2 AND admitted=1 AND operation_id!=?3)",params![request.profile,hash.as_slice(),request.id],|r|r.get(0)).map_err(|_|storage())?;
                if claimed {return Err(conflict());}
            }
            if !created {
                let stored=load_scope(c,&request.profile,&request.id)?.ok_or_else(denied)?;
                if stored!=scope {return Err(conflict());}
                return Ok(());
            }
            c.execute("INSERT INTO mtproto_operation_scopes(profile,operation_id,account_kind,account_id,action,peer_kind,peer_id,file_handle,file_sha256,file_bytes,reference_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![request.profile,request.id,account_code(scope.account),scope.account.account_id(),scope.action.name(),scope.peer.map(peer_code),scope.peer.map(PeerId::bare_id),scope.file.as_ref().map(|f|f.handle.as_str()),scope.file.as_ref().map(|f|f.sha256.as_slice()),scope.file.as_ref().map(|f|f.bytes as i64),scope.reference_hash.as_ref().map(|h|h.as_slice())]).map_err(|_|storage())?;
            Ok(())
        }).await
    }
    pub(crate) async fn begin_mtproto(
        &self,
        prepared: Prepared,
        expected: Scope,
        authorize: impl FnOnce(i64) -> Result<BudgetContext> + Send + 'static,
    ) -> Result<InFlight> {
        expected.validate()?;
        self.begin_checked(prepared,move |c,options,request,time| {
            let context=authorize(time)?;
            if context.profile!=request.profile||context.principal!=request.principal {return Err(denied());}
            let stored=load_scope(c,&request.profile,&request.id)?.ok_or_else(denied)?;
            if stored!=expected||request.action!=stored.action.name() {return Err(conflict());}
            c.execute("UPDATE mtproto_operation_scopes SET admitted=1 WHERE profile=?1 AND operation_id=?2",params![request.profile,request.id]).map_err(|e|if e.sqlite_error_code()==Some(rusqlite::ErrorCode::ConstraintViolation) {conflict()}else{storage()})?;
            charge(c,options,&context,Charge {write_attempts:1,upload_bytes:stored.file.map_or(0,|f|f.bytes),cost_units:context.cost,..Charge::default()},time)
        }).await
    }
    /// Owner-checked metadata only: no state refresh or cached JSON decoding.
    pub(crate) async fn mtproto_operation_scope(
        &self,
        profile: &str,
        principal: &str,
        id: &str,
    ) -> Result<Option<Scope>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |c,_,_| {
            let action:Option<String>=c.query_row("SELECT action FROM operations WHERE profile=?1 AND operation_id=?2 AND principal=?3",params![profile,id,principal],|r|r.get(0)).optional().map_err(|_|storage())?;
            let Some(action)=action else{return Ok(None);};
            let scope=load_scope(c,&profile,&id)?.ok_or_else(denied)?;
            if action!=scope.action.name() {return Err(storage());}
            Ok(Some(scope))
        }).await
    }
    pub(crate) async fn get_mtproto_scoped(
        &self,
        profile: &str,
        principal: &str,
        id: &str,
    ) -> Result<Option<(Operation, Scope)>> {
        for value in [profile, principal, id] {
            types::valid_id(value)?;
        }
        let (profile, principal, id) = (profile.to_owned(), principal.to_owned(), id.to_owned());
        self.run(move |c, _, _| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let Some(mut stored) = load_for(&tx, &profile, &id, Some(&principal))? else {
                return Ok(None);
            };
            let scope = load_scope(&tx, &profile, &id)?.ok_or_else(denied)?;
            if stored.request.action != scope.action.name() {
                return Err(storage());
            }
            refresh(&tx, &mut stored, now()?)?;
            tx.commit().map_err(|_| storage())?;
            Ok(Some((stored.operation, scope)))
        })
        .await
    }
}
fn load_scope(c: &Connection, profile: &str, id: &str) -> Result<Option<Scope>> {
    let scope=c.query_row("SELECT account_kind,account_id,action,peer_kind,peer_id,file_handle,file_sha256,file_bytes,reference_hash FROM mtproto_operation_scopes WHERE profile=?1 AND operation_id=?2",params![profile,id],|r| {
        let decode=||->Result<Scope> {
            let account_id=r.get::<_,i64>(1).map_err(|_|storage())?;
            let account=match r.get_ref(0).map_err(|_|storage())? {
                rusqlite::types::ValueRef::Text(b"mtproto_bot")=>ProfileIdentity::mtproto_bot(account_id),
                rusqlite::types::ValueRef::Text(b"mtproto_user")=>ProfileIdentity::mtproto_user(account_id),
                _=>return Err(storage()),
            }.map_err(|_|storage())?;
            let action=Action::ALL.into_iter().find(|a|r.get_ref(2).is_ok_and(|v|v==rusqlite::types::ValueRef::Text(a.name().as_bytes()))).ok_or_else(storage)?;
            let peer_id=r.get::<_,Option<i64>>(4).map_err(|_|storage())?;
            let kind=match r.get_ref(3).map_err(|_|storage())? {
                rusqlite::types::ValueRef::Null=>None,
                rusqlite::types::ValueRef::Text(b"user")=>Some(PeerKind::User),
                rusqlite::types::ValueRef::Text(b"chat")=>Some(PeerKind::Chat),
                rusqlite::types::ValueRef::Text(b"channel")=>Some(PeerKind::Channel),
                _=>return Err(storage()),
            };
            let peer=match (kind,peer_id) {(None,None)=>None,(Some(k),Some(id))=>Some(PeerId::new(k,id).map_err(|_|storage())?),_=>return Err(storage())};
            let handle=match r.get_ref(5).map_err(|_|storage())? {
                rusqlite::types::ValueRef::Null=>None,
                rusqlite::types::ValueRef::Text(v)=> {let v=std::str::from_utf8(v).map_err(|_|storage())?;types::valid_id(v).map_err(|_|storage())?;Some(v.to_owned())},
                _=>return Err(storage()),
            };
            let hash=types::fixed_blob::<32>(r,6).map_err(|_|storage())?;
            let bytes=r.get::<_,Option<i64>>(7).map_err(|_|storage())?;
            let file=match (handle,hash,bytes) {(None,None,None)=>None,(Some(handle),Some(sha256),Some(bytes))=>Some(FileScope{handle,sha256,bytes:u64::try_from(bytes).map_err(|_|storage())?}),_=>return Err(storage())};
            let scope=Scope{account,action,peer,file,reference_hash:types::fixed_blob(r,8).map_err(|_|storage())?};
            scope.validate().map_err(|_|storage())?;
            Ok(scope)
        };
        decode().map_err(|_|rusqlite::Error::InvalidQuery)
    }).optional().map_err(|_|storage())?;
    if let Some(scope) = &scope
        && crate::profiles::load(c, profile)? != Some(scope.account)
    {
        return Err(denied());
    }
    Ok(scope)
}

#[cfg(test)]
#[path = "mtproto_authorization_tests.rs"]
mod tests;
