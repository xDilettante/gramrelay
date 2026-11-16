//! Durable incoming updates. Storage capabilities do not grant permission to read Telegram data.
mod consumer;
mod poll;
mod types;
use crate::journal::{
    Journal, JournalError, JournalErrorKind, JournalOptions, Result, deadline, now, storage,
    types::{canonical_json, millis, valid_id},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, sync::Arc};
pub use types::*;

#[derive(Clone)]
pub struct UpdateFeed {
    journal: Journal,
    profile: String,
    bot_id: i64,
    options: Arc<UpdateOptions>,
}
impl fmt::Debug for UpdateFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateFeed").finish_non_exhaustive()
    }
}
impl UpdateFeed {
    #[cfg(feature = "executor")]
    pub(crate) async fn check_cursor_position(&self, cursor: &UpdateCursor) -> Result<()> {
        if cursor.profile != self.profile {
            return Err(JournalError::new(JournalErrorKind::Denied));
        }
        let (profile, sequence) = (self.profile.clone(), cursor.sequence);
        self.journal
            .run(move |connection, _, _| {
                let transaction = connection.transaction().map_err(|_| storage())?;
                let (last, floor) = feed_bounds(&transaction, &profile, now()?)?;
                if sequence < floor {
                    return Err(JournalError::new(JournalErrorKind::CursorExpired));
                }
                if sequence > last {
                    return Err(JournalError::new(JournalErrorKind::Validation));
                }
                transaction.commit().map_err(|_| storage())
            })
            .await
    }
    #[cfg(feature = "executor")]
    pub(crate) fn executor_binding(&self) -> (&Journal, &str, i64) {
        (&self.journal, &self.profile, self.bot_id)
    }
    #[cfg(feature = "bot")]
    pub(crate) fn bot_id(&self) -> i64 {
        self.bot_id
    }
    #[cfg(feature = "bot")]
    pub(crate) fn lease_duration(&self) -> std::time::Duration {
        self.journal.lease_duration()
    }
    pub async fn open(
        journal: Journal,
        profile: impl Into<String>,
        bot_id: i64,
        options: UpdateOptions,
    ) -> Result<Self> {
        let profile = profile.into();
        valid_id(&profile)?;
        if bot_id < 0 {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        let name = profile.clone();
        let policy = options.clone();
        journal.run(move |connection,limits,_| {
            policy.validate(limits)?;deadline(now()?,policy.retention)?;
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let existing=transaction.query_row("SELECT bot_id,retention_ms,max_event_bytes,max_batch_bytes FROM update_feeds WHERE profile=?1",[&name],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?))).optional().map_err(|_|storage())?;
            if let Some(existing)=existing {
                if existing!=(bot_id,millis(policy.retention)?,policy.max_event_bytes as i64,policy.max_batch_bytes as i64){return Err(JournalError::new(JournalErrorKind::Conflict));}
            }else{
                let alias:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM update_feeds WHERE bot_id=?1)",[bot_id],|row|row.get(0)).map_err(|_|storage())?;
                if alias{return Err(JournalError::new(JournalErrorKind::Conflict));}
                let count:i64=transaction.query_row("SELECT COUNT(*) FROM update_feeds",[],|row|row.get(0)).map_err(|_|storage())?;
                if count>=limits.max_update_feeds as i64{return Err(JournalError::new(JournalErrorKind::Capacity));}
                transaction.execute("INSERT INTO update_feeds(profile,bot_id,retention_ms,max_event_bytes,max_batch_bytes) VALUES(?1,?2,?3,?4,?5)",params![name,bot_id,millis(policy.retention)?,policy.max_event_bytes as i64,policy.max_batch_bytes as i64]).map_err(|_|storage())?;
            }
            transaction.commit().map_err(|_|storage())
        }).await?;
        Ok(Self {
            journal,
            profile,
            bot_id,
            options: Arc::new(options),
        })
    }
    pub async fn claim_source(&self, mode: SourceMode) -> Result<SourceLease> {
        let feed = self.clone();
        let duration = self.journal.lease_duration();
        self.journal.run(move |connection,_,worker|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let time=now()?;
            let (revision,until,previous):(i64,Option<i64>,Option<String>)=transaction.query_row("SELECT source_revision,source_until_ms,source_mode FROM update_feeds WHERE profile=?1",[&feed.profile],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_|storage())?;
            if until.is_some_and(|value|value>time){return Err(JournalError::new(JournalErrorKind::Capacity));}
            let revision=revision.checked_add(1).ok_or_else(storage)?;
            let until=deadline(time,duration)?;
            let changed=previous.as_deref()!=Some(mode.as_str());
            transaction.execute("UPDATE update_feeds SET source_mode=?1,source_worker=?2,source_revision=?3,source_until_ms=?4,poll_offset=CASE WHEN ?5 THEN NULL ELSE poll_offset END,poll_revision=poll_revision+?5,poll_updated_at_ms=CASE WHEN ?5 THEN NULL ELSE poll_updated_at_ms END WHERE profile=?6",params![mode.as_str(),worker,revision,until,changed,&feed.profile]).map_err(|_|storage())?;
            transaction.commit().map_err(|_|storage())?;
            Ok(SourceLease{profile:feed.profile,bot_id:feed.bot_id,revision,mode,until_ms:until,issuer:worker.into()})
        }).await
    }
    pub async fn renew_source(&self, lease: &SourceLease) -> Result<SourceLease> {
        self.renew_source_with_admission(lease, false).await
    }
    #[cfg(feature = "webhook")]
    pub(crate) async fn renew_source_fenced(&self, lease: &SourceLease) -> Result<SourceLease> {
        self.renew_source_with_admission(lease, true).await
    }
    async fn renew_source_with_admission(
        &self,
        lease: &SourceLease,
        wait: bool,
    ) -> Result<SourceLease> {
        self.check_scope(lease)?;
        let feed = self.clone();
        let lease = lease.clone();
        let duration = self.journal.lease_duration();
        let action = move |connection: &mut Connection, _: &JournalOptions, worker: &str| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| storage())?;
            let time = now()?;
            verify_source(&transaction, &feed, &lease, worker, time)?;
            let until = deadline(time, duration)?;
            transaction
                .execute(
                    "UPDATE update_feeds SET source_until_ms=?1 WHERE profile=?2",
                    params![until, feed.profile],
                )
                .map_err(|_| storage())?;
            transaction.commit().map_err(|_| storage())?;
            Ok(SourceLease {
                until_ms: until,
                ..lease
            })
        };
        if wait {
            self.journal.run_fenced(action).await
        } else {
            self.journal.run(action).await
        }
    }
    pub async fn release_source(&self, lease: SourceLease) -> Result<()> {
        self.check_scope(&lease)?;
        let name = self.profile.clone();
        self.journal.run_fenced(move |connection,_,worker|{
            if lease.issuer!=worker{return Err(lost());}
            let count=connection.execute("UPDATE update_feeds SET source_worker=NULL,source_until_ms=NULL,source_revision=source_revision+1 WHERE profile=?1 AND source_worker=?2 AND source_revision=?3",params![name,worker,lease.revision]).map_err(|_|storage())?;
            if count!=1{return Err(lost());}Ok(())
        }).await
    }
    fn check_scope(&self, lease: &SourceLease) -> Result<()> {
        if lease.profile != self.profile || lease.bot_id != self.bot_id {
            Err(lost())
        } else {
            Ok(())
        }
    }
    pub async fn append_webhook(
        &self,
        lease: &SourceLease,
        update: tg::Update,
    ) -> Result<AppendReceipt> {
        self.check_scope(lease)?;
        if lease.mode != SourceMode::Webhook {
            return Err(lost());
        }
        let event = encode_update(update, &self.options)?;
        let feed = self.clone();
        let lease = lease.clone();
        self.journal
            .run(move |connection, limits, worker| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| storage())?;
                let time = now()?;
                verify_source(&transaction, &feed, &lease, worker, time)?;
                let stored = persist_updates(
                    &transaction,
                    &feed,
                    std::slice::from_ref(&event),
                    time,
                    limits,
                )?;
                transaction.commit().map_err(|_| storage())?;
                Ok(AppendReceipt {
                    inserted: stored.inserted == 1,
                    cursor: UpdateCursor {
                        profile: feed.profile,
                        sequence: stored.positions[0],
                    },
                })
            })
            .await
    }
    pub async fn read(&self, cursor: Option<&UpdateCursor>, limit: usize) -> Result<UpdatePage> {
        self.read_with_budget(cursor, limit, self.options.max_batch_bytes)
            .await
    }
    pub(crate) async fn read_with_budget(
        &self,
        cursor: Option<&UpdateCursor>,
        limit: usize,
        byte_budget: usize,
    ) -> Result<UpdatePage> {
        if !(1..=100).contains(&limit) || byte_budget == 0 {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        if cursor.is_some_and(|cursor| cursor.profile != self.profile) {
            return Err(JournalError::new(JournalErrorKind::Denied));
        }
        let feed = self.clone();
        let cursor = cursor.cloned();
        self.journal
            .run(move |connection, _, _| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| storage())?;
                let page = read_page_bounded(
                    &transaction,
                    &feed,
                    cursor.as_ref(),
                    limit,
                    now()?,
                    byte_budget,
                )?;
                transaction.commit().map_err(|_| storage())?;
                Ok(page)
            })
            .await
    }
}
fn feed_bounds(connection: &Connection, profile: &str, time: i64) -> Result<(i64, i64)> {
    let (last, floor): (i64, i64) = connection
        .query_row(
            "SELECT last_sequence,floor_sequence FROM update_feeds WHERE profile=?1",
            [profile],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| storage())?;
    let expired: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM updates WHERE profile=?1 AND expires_at_ms<=?2",
            params![profile, time],
            |row| row.get(0),
        )
        .map_err(|_| storage())?;
    Ok((last, floor.max(expired)))
}
fn read_page(
    connection: &Connection,
    feed: &UpdateFeed,
    cursor: Option<&UpdateCursor>,
    limit: usize,
    time: i64,
) -> Result<UpdatePage> {
    read_page_bounded(
        connection,
        feed,
        cursor,
        limit,
        time,
        feed.options.max_batch_bytes,
    )
}
fn read_page_bounded(
    connection: &Connection,
    feed: &UpdateFeed,
    cursor: Option<&UpdateCursor>,
    limit: usize,
    time: i64,
    byte_budget: usize,
) -> Result<UpdatePage> {
    let byte_budget = byte_budget.min(feed.options.max_batch_bytes);
    let (last, floor) = feed_bounds(connection, &feed.profile, time)?;
    let start = cursor.map_or(floor, |cursor| cursor.sequence);
    if start < floor {
        return Err(JournalError::new(JournalErrorKind::CursorExpired));
    }
    if start > last {
        return Err(JournalError::new(JournalErrorKind::Validation));
    }
    let mut events = Vec::new();
    let mut total = 0usize;
    let mut end = start;
    let mut statement=connection.prepare("SELECT sequence,length(payload),CASE WHEN length(payload)<=?4 THEN payload ELSE NULL END,payload_hash,received_at_ms FROM updates WHERE profile=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3").map_err(|_|storage())?;
    let mut rows = statement
        .query(params![
            feed.profile,
            start,
            limit as i64,
            byte_budget as i64
        ])
        .map_err(|_| storage())?;
    while let Some(row) = rows.next().map_err(|_| storage())? {
        let sequence: i64 = row.get(0).map_err(|_| storage())?;
        let length =
            usize::try_from(row.get::<_, i64>(1).map_err(|_| storage())?).map_err(|_| storage())?;
        if length > feed.options.max_event_bytes {
            return Err(storage());
        }
        if length > byte_budget - total {
            if events.is_empty() {
                return Err(JournalError::new(JournalErrorKind::Capacity));
            }
            break;
        }
        let payload: Vec<u8> = row.get(2).map_err(|_| storage())?;
        if payload.len() != length {
            return Err(storage());
        }
        let hash: Vec<u8> = row.get(3).map_err(|_| storage())?;
        if hash.as_slice() != Sha256::digest(&payload).as_slice() {
            return Err(storage());
        }
        let update: tg::Update = serde_json::from_slice(&payload).map_err(|_| storage())?;
        total += payload.len();
        end = sequence;
        events.push(StoredUpdate {
            cursor: UpdateCursor {
                profile: feed.profile.clone(),
                sequence,
            },
            update,
            received_at_ms: row.get(4).map_err(|_| storage())?,
        });
    }
    Ok(UpdatePage {
        events,
        next_cursor: UpdateCursor {
            profile: feed.profile.clone(),
            sequence: end,
        },
        has_more: end < last,
    })
}
struct EncodedUpdate {
    id: i64,
    payload: Vec<u8>,
    hash: [u8; 32],
    chat_id: Option<i64>,
}
fn encode_update(update: tg::Update, policy: &UpdateOptions) -> Result<EncodedUpdate> {
    if update.update_id < 0 || update.known_event_count() > 1 {
        return Err(JournalError::new(JournalErrorKind::Validation));
    }
    let payload = canonical_json(&update, policy.max_event_bytes)?;
    Ok(EncodedUpdate {
        id: update.update_id,
        hash: Sha256::digest(&payload).into(),
        chat_id: update.chat_id().map(i64::from),
        payload,
    })
}
struct Persisted {
    inserted: usize,
    positions: Vec<i64>,
}
fn persist_updates(
    connection: &Connection,
    feed: &UpdateFeed,
    events: &[EncodedUpdate],
    time: i64,
    limits: &JournalOptions,
) -> Result<Persisted> {
    let mut positions = vec![0; events.len()];
    let mut fresh = Vec::new();
    let mut new_bytes = 0usize;
    for (index, event) in events.iter().enumerate() {
        let existing = connection
            .query_row(
                "SELECT sequence,payload_hash FROM updates WHERE profile=?1 AND update_id=?2",
                params![feed.profile, event.id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|_| storage())?;
        if let Some((sequence, hash)) = existing {
            if hash.as_slice() != event.hash {
                return Err(JournalError::new(JournalErrorKind::Conflict));
            }
            positions[index] = sequence;
        } else {
            fresh.push(index);
            new_bytes = new_bytes
                .checked_add(event.payload.len())
                .ok_or_else(storage)?;
        }
    }
    let (entries, bytes): (i64, i64) = connection
        .query_row(
            "SELECT COALESCE(SUM(entry_count),0),COALESCE(SUM(byte_count),0) FROM update_feeds",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| storage())?;
    if entries as u128 + fresh.len() as u128 > limits.max_update_entries as u128
        || bytes as u128 + new_bytes as u128 > limits.max_update_bytes as u128
    {
        return Err(JournalError::new(JournalErrorKind::Capacity));
    }
    if !fresh.is_empty() {
        let (mut sequence, received): (i64, i64) = connection
            .query_row(
                "SELECT last_sequence,last_received_at_ms FROM update_feeds WHERE profile=?1",
                [&feed.profile],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| storage())?;
        let received = time.max(received);
        let expires = deadline(received, feed.options.retention)?;
        for index in &fresh {
            let event = &events[*index];
            sequence = sequence.checked_add(1).ok_or_else(storage)?;
            connection.execute("INSERT INTO updates(profile,sequence,update_id,payload,payload_hash,chat_id,received_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![feed.profile,sequence,event.id,&event.payload,event.hash.as_slice(),event.chat_id,received,expires]).map_err(|_|storage())?;
            positions[*index] = sequence;
        }
        connection.execute("UPDATE update_feeds SET last_sequence=?1,last_received_at_ms=?2,entry_count=entry_count+?3,byte_count=byte_count+?4 WHERE profile=?5",params![sequence,received,fresh.len() as i64,new_bytes as i64,feed.profile]).map_err(|_|storage())?;
    }
    Ok(Persisted {
        inserted: fresh.len(),
        positions,
    })
}
fn lost() -> JournalError {
    JournalError::new(JournalErrorKind::LeaseLost)
}
fn verify_source(
    connection: &Connection,
    feed: &UpdateFeed,
    lease: &SourceLease,
    worker: &str,
    time: i64,
) -> Result<()> {
    if lease.issuer != worker {
        return Err(lost());
    }
    let valid:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM update_feeds WHERE profile=?1 AND bot_id=?2 AND source_worker=?3 AND source_revision=?4 AND source_mode=?5 AND source_until_ms>?6)",params![feed.profile,feed.bot_id,worker,lease.revision,lease.mode.as_str(),time],|row|row.get(0)).map_err(|_|storage())?;
    if valid { Ok(()) } else { Err(lost()) }
}
impl Journal {
    /// Delete only expired inbound rows. Operation-key retention is independent and unchanged.
    pub async fn prune_updates(&self, limit: usize) -> Result<usize> {
        if !(1..=1000).contains(&limit) {
            return Err(JournalError::new(JournalErrorKind::Validation));
        }
        self.run(move |connection,_,_|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            let expired={
                let mut statement=transaction.prepare("SELECT profile,sequence,length(payload) FROM updates WHERE expires_at_ms<=?1 ORDER BY expires_at_ms,profile,sequence LIMIT ?2").map_err(|_|storage())?;
                statement.query_map(params![now()?,limit as i64],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?))).map_err(|_|storage())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|storage())?
            };
            let mut groups:BTreeMap<String,(i64,i64,i64)>=BTreeMap::new();
            for (profile,sequence,bytes) in &expired{
                transaction.execute("DELETE FROM updates WHERE profile=?1 AND sequence=?2",params![profile,sequence]).map_err(|_|storage())?;
                let group=groups.entry(profile.clone()).or_default();group.0+=1;group.1+=bytes;group.2=group.2.max(*sequence);
            }
            for (profile,(count,bytes,floor)) in groups{
                transaction.execute("UPDATE update_feeds SET entry_count=entry_count-?1,byte_count=byte_count-?2,floor_sequence=max(floor_sequence,?3) WHERE profile=?4",params![count,bytes,floor,profile]).map_err(|_|storage())?;
            }
            transaction.commit().map_err(|_|storage())?;Ok(expired.len())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "webhook")]
    #[tokio::test]
    async fn heartbeat_admission_waits_behind_cancelled_native_work() {
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
        let lease = feed.claim_source(SourceMode::Webhook).await.unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = journal.clone();
        let task = tokio::spawn(async move {
            worker
                .run(move |_, _, _| {
                    let _ = entered.send(());
                    blocked
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .map_err(|_| storage())?;
                    Ok(())
                })
                .await
        });
        waiting.await.unwrap();
        task.abort();
        let renewal = feed.renew_source_fenced(&lease);
        tokio::pin!(renewal);
        tokio::select! {biased;result=&mut renewal=>panic!("maintenance must wait: {result:?}"),_=std::future::ready(())=>{}}
        release.send(()).unwrap();
        renewal.await.unwrap();
        assert!(task.await.unwrap_err().is_cancelled());
        journal.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_well_formed_future_cursor_is_rejected_without_reading_or_advancing() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("updates.sqlite"),
            crate::journal::JournalOptions::default(),
        )
        .await
        .unwrap();
        let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        let cursor = UpdateCursor {
            profile: "bot".into(),
            sequence: i64::MAX,
        };
        let cursor: UpdateCursor = cursor.to_string().parse().unwrap();
        assert_eq!(
            feed.read(Some(&cursor), 100).await.unwrap_err().kind(),
            JournalErrorKind::Validation
        );
        assert!(feed.read(None, 100).await.unwrap().events().is_empty());
        journal.close().await.unwrap();
    }
}
