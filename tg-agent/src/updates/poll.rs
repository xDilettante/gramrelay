use super::*;

impl UpdateFeed {
    pub async fn poll_checkpoint(&self, lease: &SourceLease) -> Result<PollCheckpoint> {
        self.check_scope(lease)?;
        if lease.mode != SourceMode::Polling {
            return Err(lost());
        }
        let feed = self.clone();
        let lease = lease.clone();
        self.journal.run(move |connection,_,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;verify_source(&transaction,&feed,&lease,worker,time)?;
            let (offset,revision,updated,until):(Option<i64>,i64,Option<i64>,i64)=transaction.query_row(
                "SELECT poll_offset,poll_revision,poll_updated_at_ms,source_until_ms FROM update_feeds WHERE profile=?1",[&feed.profile],
                |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(|_|storage())?;
            let fresh=updated.and_then(|updated|time.checked_sub(updated)).is_some_and(|age|(0..86_400_000).contains(&age));
            transaction.commit().map_err(|_|storage())?;
            Ok(PollCheckpoint{profile:feed.profile,bot_id:feed.bot_id,source_revision:lease.revision,revision,
                offset:offset.filter(|_|fresh),issuer:worker.into(),until_ms:until})
        }).await
    }

    pub async fn append_poll(
        &self,
        lease: &SourceLease,
        checkpoint: &PollCheckpoint,
        updates: Vec<tg::Update>,
    ) -> Result<PollReceipt> {
        self.check_scope(lease)?;
        if lease.mode != SourceMode::Polling
            || checkpoint.profile != self.profile
            || checkpoint.bot_id != self.bot_id
            || checkpoint.source_revision != lease.revision
        {
            return Err(lost());
        }
        if updates.len() > 100
            || updates
                .windows(2)
                .any(|pair| pair[0].update_id >= pair[1].update_id)
            || updates.first().is_some_and(|update| {
                checkpoint
                    .offset
                    .is_some_and(|offset| update.update_id < offset)
            })
        {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        let next = updates
            .last()
            .map(|update| {
                update
                    .update_id
                    .checked_add(1)
                    .ok_or_else(|| JournalError::new(JournalErrorKind::Validation))
            })
            .transpose()?;
        let mut events = Vec::with_capacity(updates.len());
        let mut bytes = 0usize;
        for update in updates {
            let event = encode_update(update, &self.options)?;
            bytes = bytes.checked_add(event.payload.len()).ok_or_else(storage)?;
            if bytes > self.options.max_batch_bytes {
                return Err(JournalError::new(JournalErrorKind::Capacity));
            }
            events.push(event);
        }
        let feed = self.clone();
        let lease = lease.clone();
        let checkpoint = checkpoint.clone();
        self.journal.run(move |connection,limits,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;verify_source(&transaction,&feed,&lease,worker,time)?;
            if checkpoint.issuer!=worker||checkpoint.until_ms<=time{return Err(lost());}
            let revision:i64=transaction.query_row("SELECT poll_revision FROM update_feeds WHERE profile=?1",[&feed.profile],|row|row.get(0)).map_err(|_|storage())?;
            if revision!=checkpoint.revision{return Err(JournalError::new(JournalErrorKind::Conflict));}
            let stored=persist_updates(&transaction,&feed,&events,time,limits)?;
            if let Some(next)=next {
                let revision=revision.checked_add(1).ok_or_else(storage)?;
                transaction.execute("UPDATE update_feeds SET poll_offset=?1,poll_revision=?2,poll_updated_at_ms=?3 WHERE profile=?4",params![next,revision,time,feed.profile]).map_err(|_|storage())?;
            }
            transaction.commit().map_err(|_|storage())?;
            Ok(PollReceipt{inserted:stored.inserted,duplicates:events.len()-stored.inserted,next_offset:next.or(checkpoint.offset)})
        }).await
    }
}
