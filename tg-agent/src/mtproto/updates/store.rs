use super::*;
use project::{Encoded, Payload, Stamp};
pub(super) enum Origin {
    Batch(Stamp),
    SourceChange([u8; 32]),
}
// Borrow fixed metadata before copying it; SQLite row decoding never allocates
// a Vec for a source ID, ordinal, batch ID, hash or private query ID.
pub(super) fn blob<const N: usize>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<[u8; N]>> {
    match row.get_ref(index)? {
        rusqlite::types::ValueRef::Null => Ok(None),
        rusqlite::types::ValueRef::Blob(bytes) => bytes
            .try_into()
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
struct Watermark {
    source: Option<[u8; 32]>,
    ordinal: Option<u64>,
    id: Option<[u8; 32]>,
    hash: Option<[u8; 32]>,
    kind: Option<&'static str>,
    start: Option<i64>,
    end: Option<i64>,
}
fn watermark(c: &Connection, profile: &str) -> Result<Watermark> {
    c.query_row("SELECT source_id,last_ordinal,last_id,last_hash,last_kind,last_start,last_end FROM mtproto_update_feeds WHERE profile=?1",[profile],|row| {
        let kind=match row.get_ref(4)? {
            rusqlite::types::ValueRef::Null=>None,
            rusqlite::types::ValueRef::Text(b"baseline")=>Some("baseline"),
            rusqlite::types::ValueRef::Text(b"events")=>Some("events"),
            rusqlite::types::ValueRef::Text(b"resync")=>Some("resync"),
            rusqlite::types::ValueRef::Text(b"gap")=>Some("gap"),
            _=>return Err(rusqlite::Error::InvalidQuery),
        };
        Ok(Watermark {source:blob(row,0)?,ordinal:blob(row,1)?.map(u64::from_le_bytes),id:blob(row,2)?,hash:blob(row,3)?,kind,start:row.get(5)?,end:row.get(6)?})
    }).map_err(|_|storage())
}
pub(super) fn insert(
    c: &Connection,
    feed: &Feed,
    origin: Origin,
    events: &[Encoded],
    limits: &JournalOptions,
) -> Result<(i64, i64)> {
    let (source, ordinal, id, kind) = match origin {
        Origin::Batch(v) => (v.source, Some(v.ordinal.to_le_bytes()), Some(v.id), v.kind),
        Origin::SourceChange(id) => (id, None, None, "source_change"),
    };
    if events.is_empty() || events.len() > 4096 {
        return Err(invalid());
    }
    let added = events
        .iter()
        .try_fold(0usize, |sum, e| {
            sum.checked_add(e.payload.len() + e.query.map_or(0, |_| 8))
        })
        .ok_or_else(capacity)?;
    if added > feed.options.max_batch_bytes {
        return Err(capacity());
    }
    let (entries, bytes) = crate::updates::storage_usage(c)?;
    if entries as u128 + events.len() as u128 > limits.max_update_entries as u128
        || bytes as u128 + added as u128 > limits.max_update_bytes as u128
    {
        return Err(capacity());
    }
    let (mut seq, previous): (i64, i64) = c
        .query_row(
            "SELECT last_sequence,last_received_at_ms FROM mtproto_update_feeds WHERE profile=?1",
            [&feed.profile],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| storage())?;
    let start = seq.checked_add(1).ok_or_else(storage)?;
    let time = now()?.max(previous);
    let expires = deadline(time, feed.options.retention)?;
    for event in events {
        seq = seq.checked_add(1).ok_or_else(storage)?;
        let query = event.query.map(i64::to_le_bytes);
        let bytes = event.payload.len() + query.as_ref().map_or(0, |v| v.len());
        if bytes > feed.options.max_event_bytes
            || project::digest(&event.payload, event.query) != event.hash
        {
            return Err(invalid());
        }
        c.execute("INSERT INTO mtproto_updates(profile,sequence,source_id,ordinal,batch_id,batch_kind,payload,payload_hash,callback_query,byte_size,received_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![feed.profile,seq,source.as_slice(),ordinal.as_ref().map(|v|v.as_slice()),id.as_ref().map(|v|v.as_slice()),kind,event.payload,event.hash.as_slice(),query.as_ref().map(|v|v.as_slice()),bytes as i64,time,expires]).map_err(|_|storage())?;
    }
    c.execute("UPDATE mtproto_update_feeds SET last_sequence=?1,last_received_at_ms=?2,entry_count=entry_count+?3,byte_count=byte_count+?4 WHERE profile=?5",params![seq,time,events.len() as i64,added as i64,feed.profile]).map_err(|_|storage())?;
    Ok((start, seq))
}
// Metadata-only cursor validation is shared by dry-run and actual payload reads.
fn cursor_start(c: &Connection, feed: &Feed, cursor: Option<&Cursor>) -> Result<(i64, i64)> {
    if cursor.is_some_and(|v| v.profile != feed.profile) {
        return Err(JournalError::new(JournalErrorKind::Denied));
    }
    if crate::profiles::load(c, &feed.profile)? != Some(feed.account) {
        return Err(lost());
    }
    let (last, saved_floor): (i64, i64) = c
        .query_row(
            "SELECT last_sequence,floor_sequence FROM mtproto_update_feeds WHERE profile=?1",
            [&feed.profile],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| storage())?;
    let expired: i64 = c.query_row(
        "SELECT COALESCE(MAX(sequence),0) FROM mtproto_updates WHERE profile=?1 AND expires_at_ms<=?2",
        params![feed.profile,now()?], |r| r.get(0),
    ).map_err(|_| storage())?;
    let floor = saved_floor.max(expired);
    let end = cursor.map_or(floor, |v| v.sequence);
    if end < floor {
        return Err(JournalError::new(JournalErrorKind::CursorExpired));
    }
    if end > last {
        return Err(invalid());
    }
    Ok((end, last))
}
impl Feed {
    pub(super) async fn check_cursor_position(&self, cursor: Option<&Cursor>) -> Result<()> {
        let feed = self.clone();
        let cursor = cursor.cloned();
        self.journal
            .run(move |c, _, _| feed.check_cursor_stored(c, cursor.as_ref()))
            .await
    }
    pub(super) fn check_cursor_stored(
        &self,
        c: &mut Connection,
        cursor: Option<&Cursor>,
    ) -> Result<()> {
        let tx = c.transaction().map_err(|_| storage())?;
        cursor_start(&tx, self, cursor)?;
        tx.commit().map_err(|_| storage())
    }
    pub(super) async fn append(
        &self,
        lease: &SourceLease,
        stamp: Stamp,
        events: Vec<Encoded>,
        observed: Instant,
    ) -> Result<Receipt> {
        self.check_scope(lease)?;
        if stamp.ordinal == 0 || !matches!(stamp.kind, "baseline" | "events" | "resync" | "gap") {
            return Err(invalid());
        }
        let feed = self.clone();
        let lease = lease.clone();
        let tracker = self.journal.callback_receipts.clone();
        self.journal.run(move |c,limits,worker| {
            let mut hash=Sha256::new();hash.update(b"tg-agent/mtproto/batch/v1\0");hash.update(stamp.kind.as_bytes());hash.update((events.len() as u64).to_le_bytes());for event in &events {hash.update(event.hash);}let hash:[u8;32]=hash.finalize().into();
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;feed.verify(&tx,&lease,worker)?;
            let old=watermark(&tx,&feed.profile)?;
            if old.source.is_some_and(|source|source!=stamp.source) {return Err(conflict());}
            if let Some(ordinal)=old.ordinal && ordinal>=stamp.ordinal {
                    if ordinal!=stamp.ordinal||old.id!=Some(stamp.id)||old.hash!=Some(hash)||old.kind!=Some(stamp.kind) {return Err(conflict());}
                    let receipt=Receipt {inserted:false,start:old.start.ok_or_else(storage)?,end:old.end.ok_or_else(storage)?};
                    tx.commit().map_err(|_|storage())?;source::still_live(&lease)?;return Ok(receipt);
            }
            let (start,end)=insert(&tx,&feed,Origin::Batch(stamp),&events,limits)?;
            tx.execute("UPDATE mtproto_update_feeds SET source_id=?1,last_ordinal=?2,last_id=?3,last_hash=?4,last_kind=?5,last_start=?6,last_end=?7 WHERE profile=?8",params![stamp.source.as_slice(),stamp.ordinal.to_le_bytes().as_slice(),stamp.id.as_slice(),hash.as_slice(),stamp.kind,start,end,feed.profile]).map_err(|_|storage())?;
            tx.commit().map_err(|_|storage())?;source::still_live(&lease)?;
            let positions:Vec<_>=events.iter().enumerate().filter(|(_,e)|e.query.is_some()).map(|(i,_)|start+i as i64).collect();
            tracker.record(&feed.profile,&positions,observed);
            Ok(Receipt {inserted:true,start,end})
        }).await
    }
    pub(super) async fn confirm(&self, source: [u8; 32], id: [u8; 32]) -> Result<()> {
        let feed = self.clone();
        self.journal.run_fenced(move |c,_,_| {
            if crate::profiles::load(c,&feed.profile)?!=Some(feed.account) {return Err(lost());}
            c.execute("UPDATE mtproto_updates SET checkpoint_acknowledged=1 WHERE profile=?1 AND source_id=?2 AND batch_id=?3 AND batch_kind IN ('baseline','events','resync')",params![feed.profile,source.as_slice(),id.as_slice()]).map_err(|_|storage())?;Ok(())
        }).await
    }
    /// Trusted storage read. The executor must check current grant per event.
    pub async fn read(&self, cursor: Option<&Cursor>, limit: usize) -> Result<Page> {
        self.read_with_budget(cursor, limit, self.options.max_batch_bytes)
            .await
    }
    pub(crate) async fn read_with_budget(
        &self,
        cursor: Option<&Cursor>,
        limit: usize,
        budget: usize,
    ) -> Result<Page> {
        if !(1..=100).contains(&limit) || budget == 0 {
            return Err(invalid());
        }
        if cursor.is_some_and(|c| c.profile != self.profile) {
            return Err(JournalError::new(JournalErrorKind::Denied));
        }
        let feed = self.clone();
        let cursor = cursor.cloned();
        self.journal
            .run(move |c, _, _| feed.read_stored(c, cursor.as_ref(), limit, budget))
            .await
    }
    pub(super) fn read_stored(
        &self,
        c: &mut Connection,
        cursor: Option<&Cursor>,
        limit: usize,
        budget: usize,
    ) -> Result<Page> {
        let tracker = &self.journal.callback_receipts;
        let origin = self.journal.origin_token();
        let tx = c.transaction().map_err(|_| storage())?;
        let (mut end, last) = cursor_start(&tx, self, cursor)?;
        let mut statement=tx.prepare("SELECT sequence,byte_size,length(payload),payload,payload_hash,callback_query,received_at_ms,checkpoint_acknowledged FROM mtproto_updates WHERE profile=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3").map_err(|_|storage())?;
        let mut rows = statement
            .query(params![self.profile, end, limit as i64])
            .map_err(|_| storage())?;
        let mut used = 0usize;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(|_| storage())? {
            let size = usize::try_from(row.get::<_, i64>(1).map_err(|_| storage())?)
                .map_err(|_| storage())?;
            let length = usize::try_from(row.get::<_, i64>(2).map_err(|_| storage())?)
                .map_err(|_| storage())?;
            if size > self.options.max_event_bytes || length > size {
                return Err(storage());
            }
            if size > budget.saturating_sub(used) {
                if events.is_empty() {
                    return Err(capacity());
                }
                break;
            }
            let payload: Vec<u8> = row.get(3).map_err(|_| storage())?;
            let hash: [u8; 32] = blob(row, 4).map_err(|_| storage())?.ok_or_else(storage)?;
            let query = blob(row, 5).map_err(|_| storage())?.map(i64::from_le_bytes);
            if payload.len() != length
                || size != length + query.map_or(0, |_| 8)
                || hash != project::digest(&payload, query)
            {
                return Err(storage());
            }
            let decoded: Payload = serde_json::from_slice(&payload).map_err(|_| storage())?;
            if decoded.schema_version != 1
                || matches!(decoded.event, Event::Callback { .. }) != query.is_some()
            {
                return Err(storage());
            }
            let seq: i64 = row.get(0).map_err(|_| storage())?;
            events.push(StoredEvent {
                cursor: Cursor {
                    profile: self.profile.clone(),
                    sequence: seq,
                },
                event: decoded.event,
                received_at_ms: row.get(6).map_err(|_| storage())?,
                checkpoint_acknowledged: row.get(7).map_err(|_| storage())?,
                callback_query: query,
                origin: origin.clone(),
                callback_received: tracker.get(&self.profile, seq),
            });
            used += size;
            end = seq;
        }
        drop(rows);
        drop(statement);
        tx.commit().map_err(|_| storage())?;
        Ok(Page {
            events,
            cursor: Cursor {
                profile: self.profile.clone(),
                sequence: end,
            },
            has_more: end < last,
        })
    }
}
