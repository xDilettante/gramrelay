use super::*;
fn local_live(lease: &SourceLease) -> Result<()> {
    if Instant::now() >= lease.deadline || now()? >= lease.until_ms {
        Err(lost())
    } else {
        Ok(())
    }
}
impl Feed {
    pub(super) fn check_scope(&self, lease: &SourceLease) -> Result<()> {
        if lease.profile != self.profile || lease.account != self.account {
            Err(lost())
        } else {
            Ok(())
        }
    }
    pub(super) fn verify(&self, c: &Connection, lease: &SourceLease, worker: &str) -> Result<()> {
        self.check_scope(lease)?;
        local_live(lease)?;
        if lease.issuer != worker || crate::profiles::load(c, &self.profile)? != Some(self.account)
        {
            return Err(lost());
        }
        let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM mtproto_update_feeds WHERE profile=?1 AND source_worker=?2 AND source_revision=?3 AND source_until_ms>?4)",params![self.profile,worker,lease.revision,now()?],|r|r.get(0)).map_err(|_|storage())?;
        if valid { Ok(()) } else { Err(lost()) }
    }
    pub async fn claim_source(&self) -> Result<SourceLease> {
        let feed = self.clone();
        let duration = self.journal.lease_duration();
        self.journal.run(move |c,_,worker| {
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            if crate::profiles::load(&tx,&feed.profile)?!=Some(feed.account) {return Err(lost());}
            let (revision,until):(i64,Option<i64>)=tx.query_row("SELECT source_revision,source_until_ms FROM mtproto_update_feeds WHERE profile=?1",[&feed.profile],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|storage())?;
            let sampled=Instant::now();let time=now()?;
            if until.is_some_and(|until|until>time) {return Err(capacity());}
            let revision=revision.checked_add(1).ok_or_else(storage)?;
            let lease=SourceLease {profile:feed.profile.clone(),account:feed.account,issuer:worker.to_owned(),revision,until_ms:deadline(time,duration)?,deadline:sampled.checked_add(duration).ok_or_else(invalid)?};
            tx.execute("UPDATE mtproto_update_feeds SET source_worker=?1,source_revision=?2,source_until_ms=?3 WHERE profile=?4",params![worker,revision,lease.until_ms,feed.profile]).map_err(|_|storage())?;
            tx.commit().map_err(|_|storage())?;local_live(&lease)?;Ok(lease)
        }).await
    }
    pub async fn renew_source(&self, lease: &SourceLease) -> Result<SourceLease> {
        self.check_scope(lease)?;
        let feed = self.clone();
        let mut lease = lease.clone();
        let duration = self.journal.lease_duration();
        self.journal
            .run_fenced(move |c, _, worker| {
                let tx = c
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| storage())?;
                feed.verify(&tx, &lease, worker)?;
                let sampled = Instant::now();
                lease.until_ms = deadline(now()?, duration)?;
                lease.deadline = sampled.checked_add(duration).ok_or_else(invalid)?;
                tx.execute(
                    "UPDATE mtproto_update_feeds SET source_until_ms=?1 WHERE profile=?2",
                    params![lease.until_ms, feed.profile],
                )
                .map_err(|_| storage())?;
                tx.commit().map_err(|_| storage())?;
                local_live(&lease)?;
                Ok(lease)
            })
            .await
    }
    pub async fn release_source(&self, lease: &SourceLease) -> Result<()> {
        self.check_scope(lease)?;
        let feed = self.clone();
        let lease = lease.clone();
        self.journal.run_fenced(move |c,_,worker| {
            if lease.issuer!=worker {return Err(lost());}
            let count=c.execute("UPDATE mtproto_update_feeds SET source_worker=NULL,source_until_ms=NULL,source_revision=source_revision+1 WHERE profile=?1 AND source_worker=?2 AND source_revision=?3",params![feed.profile,worker,lease.revision]).map_err(|_|storage())?;
            if count==1 {Ok(())} else {Err(lost())}
        }).await
    }
    pub(super) async fn check_source(&self, lease: &SourceLease, source: [u8; 32]) -> Result<()> {
        let feed = self.clone();
        let lease = lease.clone();
        self.journal
            .run(move |c, _, worker| {
                feed.verify(c, &lease, worker)?;
                let saved: Option<[u8; 32]> = c
                    .query_row(
                        "SELECT source_id FROM mtproto_update_feeds WHERE profile=?1",
                        [&feed.profile],
                        |r| store::blob(r, 0),
                    )
                    .map_err(|_| storage())?;
                if saved.is_some_and(|v| v != source) {
                    Err(conflict())
                } else {
                    Ok(())
                }
            })
            .await
    }
    /// Explicit host discontinuity decision. Join the old Client first. This
    /// preserves old events, clears only the source watermark and rotates lease.
    pub async fn accept_source_change(
        &self,
        lease: &SourceLease,
        receiver: &tg_mtproto::UpdateReceiver,
    ) -> Result<SourceLease> {
        self.check_scope(lease)?;
        if project::identity(receiver.identity())? != self.account {
            return Err(invalid());
        }
        let source = receiver.source_id();
        let feed = self.clone();
        let lease = lease.clone();
        let duration = self.journal.lease_duration();
        self.journal.run(move |c,limits,worker| {
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;feed.verify(&tx,&lease,worker)?;
            let old:Option<[u8;32]>=tx.query_row("SELECT source_id FROM mtproto_update_feeds WHERE profile=?1",[&feed.profile],|r|store::blob(r,0)).map_err(|_|storage())?;
            if old==Some(source) {return Err(conflict());}
            if old.is_none() {return Err(invalid());}
            let event=project::encode(Event::SourceChanged,None,&feed.options)?;
            store::insert(&tx,&feed,store::Origin::SourceChange(source),&[event],limits)?;
            let sampled=Instant::now();let next=SourceLease {revision:lease.revision.checked_add(1).ok_or_else(storage)?,until_ms:deadline(now()?,duration)?,deadline:sampled.checked_add(duration).ok_or_else(invalid)?,..lease};
            tx.execute("UPDATE mtproto_update_feeds SET source_id=?1,last_ordinal=NULL,last_id=NULL,last_hash=NULL,last_kind=NULL,last_start=NULL,last_end=NULL,source_revision=?2,source_until_ms=?3 WHERE profile=?4",params![source.as_slice(),next.revision,next.until_ms,feed.profile]).map_err(|_|storage())?;
            tx.commit().map_err(|_|storage())?;local_live(&next)?;Ok(next)
        }).await
    }
}
pub(super) fn still_live(lease: &SourceLease) -> Result<()> {
    local_live(lease)
}
