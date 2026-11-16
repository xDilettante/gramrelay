use super::*;

impl UpdateFeed {
    pub async fn claim_consumer(&self, id: impl Into<String>) -> Result<ConsumerLease> {
        let id = id.into();
        valid_id(&id)?;
        let feed = self.clone();
        let duration = self.journal.lease_duration();
        self.journal.run(move |connection,limits,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;let until=deadline(time,duration)?;
            let existing=transaction.query_row("SELECT revision,lease_until_ms FROM update_consumers WHERE profile=?1 AND consumer_id=?2",params![feed.profile,id],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<i64>>(1)?))).optional().map_err(|_|storage())?;
            let revision=if let Some((revision,expires))=existing{
                if expires.is_some_and(|value|value>time){return Err(JournalError::new(JournalErrorKind::Capacity));}
                let revision=revision.checked_add(1).ok_or_else(storage)?;
                transaction.execute("UPDATE update_consumers SET worker=?1,revision=?2,lease_until_ms=?3 WHERE profile=?4 AND consumer_id=?5",params![worker,revision,until,feed.profile,id]).map_err(|_|storage())?;revision
            }else{
                let count:i64=transaction.query_row("SELECT COUNT(*) FROM update_consumers",[],|row|row.get(0)).map_err(|_|storage())?;
                if count>=limits.max_update_consumers as i64{return Err(JournalError::new(JournalErrorKind::Capacity));}
                let (_,floor)=feed_bounds(&transaction,&feed.profile,time)?;
                transaction.execute("INSERT INTO update_consumers(profile,consumer_id,position,worker,revision,lease_until_ms) VALUES(?1,?2,?3,?4,1,?5)",params![feed.profile,id,floor,worker,until]).map_err(|_|storage())?;1
            };
            transaction.commit().map_err(|_|storage())?;
            Ok(ConsumerLease{profile:feed.profile,bot_id:feed.bot_id,consumer_id:id,revision,issuer:worker.into(),until_ms:until})
        }).await
    }

    pub async fn renew_consumer(&self, lease: &ConsumerLease) -> Result<ConsumerLease> {
        self.check_consumer(lease)?;
        let feed = self.clone();
        let lease = lease.clone();
        let duration = self.journal.lease_duration();
        self.journal.run(move |connection,_,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;verify_consumer(&transaction,&lease,worker,time)?;let until=deadline(time,duration)?;
            transaction.execute("UPDATE update_consumers SET lease_until_ms=?1 WHERE profile=?2 AND consumer_id=?3",params![until,feed.profile,lease.consumer_id]).map_err(|_|storage())?;
            transaction.commit().map_err(|_|storage())?;Ok(ConsumerLease{until_ms:until,..lease})
        }).await
    }
    pub async fn release_consumer(&self, lease: ConsumerLease) -> Result<()> {
        self.check_consumer(&lease)?;
        self.journal.run(move |connection,_,worker|{
            if lease.issuer!=worker{return Err(lost());}
            let count=connection.execute("UPDATE update_consumers SET worker=NULL,lease_until_ms=NULL,revision=revision+1 WHERE profile=?1 AND consumer_id=?2 AND worker=?3 AND revision=?4",params![lease.profile,lease.consumer_id,worker,lease.revision]).map_err(|_|storage())?;
            if count==1{Ok(())}else{Err(lost())}
        }).await
    }
    pub async fn read_consumer(
        &self,
        lease: &ConsumerLease,
        limit: usize,
    ) -> Result<ConsumerBatch> {
        self.check_consumer(lease)?;
        if !(1..=100).contains(&limit) {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        let feed = self.clone();
        let lease = lease.clone();
        self.journal
            .run(move |connection, _, worker| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| storage())?;
                let time = now()?;
                let start = verify_consumer(&transaction, &lease, worker, time)?;
                let page = read_page(
                    &transaction,
                    &feed,
                    Some(&UpdateCursor {
                        profile: feed.profile.clone(),
                        sequence: start,
                    }),
                    limit,
                    time,
                )?;
                let acknowledgement = ConsumerAck {
                    lease,
                    start,
                    end: page.next_cursor.sequence,
                };
                transaction.commit().map_err(|_| storage())?;
                Ok(ConsumerBatch {
                    page,
                    acknowledgement,
                })
            })
            .await
    }
    pub async fn acknowledge_consumer(
        &self,
        lease: &ConsumerLease,
        ack: ConsumerAck,
    ) -> Result<UpdateCursor> {
        self.check_consumer(lease)?;
        if lease.profile != ack.lease.profile
            || lease.bot_id != ack.lease.bot_id
            || lease.consumer_id != ack.lease.consumer_id
            || lease.revision != ack.lease.revision
            || lease.issuer != ack.lease.issuer
        {
            return Err(lost());
        }
        let lease = lease.clone();
        self.journal.run(move |connection,_,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let position=verify_consumer(&transaction,&lease,worker,now()?)?;
            if position!=ack.start{return Err(JournalError::new(JournalErrorKind::Conflict));}
            transaction.execute("UPDATE update_consumers SET position=?1 WHERE profile=?2 AND consumer_id=?3",params![ack.end,lease.profile,lease.consumer_id]).map_err(|_|storage())?;
            transaction.commit().map_err(|_|storage())?;Ok(UpdateCursor{profile:lease.profile,sequence:ack.end})
        }).await
    }
    /// Explicit host decision after CursorExpired; never skips still-retained unread rows.
    pub async fn skip_expired_consumer(&self, lease: &ConsumerLease) -> Result<UpdateCursor> {
        self.check_consumer(lease)?;
        let feed = self.clone();
        let lease = lease.clone();
        self.journal.run(move |connection,_,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;let position=verify_consumer(&transaction,&lease,worker,time)?;let (_,floor)=feed_bounds(&transaction,&feed.profile,time)?;let position=position.max(floor);
            transaction.execute("UPDATE update_consumers SET position=?1 WHERE profile=?2 AND consumer_id=?3",params![position,lease.profile,lease.consumer_id]).map_err(|_|storage())?;
            transaction.commit().map_err(|_|storage())?;Ok(UpdateCursor{profile:lease.profile,sequence:position})
        }).await
    }
    fn check_consumer(&self, lease: &ConsumerLease) -> Result<()> {
        if lease.profile != self.profile || lease.bot_id != self.bot_id {
            Err(lost())
        } else {
            Ok(())
        }
    }
}
fn verify_consumer(
    connection: &Connection,
    lease: &ConsumerLease,
    worker: &str,
    time: i64,
) -> Result<i64> {
    if lease.issuer != worker {
        return Err(lost());
    }
    connection.query_row("SELECT position FROM update_consumers WHERE profile=?1 AND consumer_id=?2 AND worker=?3 AND revision=?4 AND lease_until_ms>?5",params![lease.profile,lease.consumer_id,worker,lease.revision,time],|row|row.get(0)).optional().map_err(|_|storage())?.ok_or_else(lost)
}
