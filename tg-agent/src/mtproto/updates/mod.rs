//! Trusted durable intake. Storage access and event text never grant agent authority.
//!
//! A host keeps one receiver/lease and reconstructs or accepts skipped history
//! explicitly when a gap is returned. `ingest` commits projected records before
//! M2 ACK. Errors can follow a commit; repeat through the same durable feed.
//!
//! ```no_run
//! use std::time::Duration;
//! use tg_agent::mtproto::updates::{Feed, SourceLease, Ingested, IngestError};
//! use tg_mtproto::{UpdateReceiver, ResyncRequired};
//!
//! async fn collect_one(feed: &Feed, source: &mut SourceLease,
//!     receiver: &mut UpdateReceiver) -> Result<Option<ResyncRequired>, IngestError> {
//!     match feed.ingest(source, receiver, Duration::from_secs(1)).await? {
//!         Ingested::Gap(required) => Ok(Some(required)),
//!         Ingested::Batch(_) | Ingested::Idle => Ok(None),
//!     }
//! }
//! ```
mod project;
mod source;
mod store;
#[cfg(test)]
mod tests;
mod types;
use super::{PeerId, PeerKind};
pub use crate::updates::UpdateOptions as Options;
use crate::{
    journal::{
        Journal, JournalError, JournalErrorKind, JournalOptions, Result, deadline, now, storage,
        types::{millis, valid_id},
    },
    profiles::{ProfileIdentity, ProfileKind},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub use types::*;

#[derive(Clone)]
pub struct Feed {
    journal: Journal,
    profile: String,
    account: ProfileIdentity,
    options: Arc<Options>,
}
impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MtprotoFeed([REDACTED])")
    }
}
fn invalid() -> JournalError {
    JournalError::new(JournalErrorKind::Validation)
}
fn capacity() -> JournalError {
    JournalError::new(JournalErrorKind::Capacity)
}
fn conflict() -> JournalError {
    JournalError::new(JournalErrorKind::Conflict)
}
fn lost() -> JournalError {
    JournalError::new(JournalErrorKind::LeaseLost)
}
impl Feed {
    /// Host registration only; the M2 receiver independently verifies live self.
    pub async fn open(
        journal: Journal,
        profile: impl Into<String>,
        account: ProfileIdentity,
        options: Options,
    ) -> Result<Self> {
        if !matches!(
            account.kind(),
            ProfileKind::MtprotoBot | ProfileKind::MtprotoUser
        ) {
            return Err(invalid());
        }
        let profile = profile.into();
        valid_id(&profile)?;
        let name = profile.clone();
        let policy = options.clone();
        journal.run(move |c,limits,_| {
            policy.validate(limits)?;deadline(now()?,policy.retention)?;
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
            crate::profiles::bind(&tx,limits,&name,account)?;
            let old:Option<(i64,i64,i64)>=tx.query_row("SELECT retention_ms,max_event_bytes,max_batch_bytes FROM mtproto_update_feeds WHERE profile=?1",[&name],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|storage())?;
            let config=(millis(policy.retention)?,policy.max_event_bytes as i64,policy.max_batch_bytes as i64);
            if let Some(old)=old {if old!=config {return Err(conflict());}} else {
                if crate::updates::feed_count(&tx)? >= limits.max_update_feeds as i64 {return Err(capacity());}
                tx.execute("INSERT INTO mtproto_update_feeds(profile,retention_ms,max_event_bytes,max_batch_bytes) VALUES(?1,?2,?3,?4)",params![name,config.0,config.1,config.2]).map_err(|_|storage())?;
            }
            tx.commit().map_err(|_|storage())
        }).await?;
        Ok(Self {
            journal,
            profile,
            account,
            options: Arc::new(options),
        })
    }
    pub fn identity(&self) -> ProfileIdentity {
        self.account
    }
    /// Host-controlled append-before-ACK boundary. This never acknowledges M2.
    pub async fn store_batch(
        &self,
        lease: &SourceLease,
        batch: &tg_mtproto::UpdateBatch,
    ) -> Result<Receipt> {
        if project::identity(batch.identity())? != self.account {
            return Err(invalid());
        }
        let observed = Instant::now();
        let (stamp, events) = project::batch(batch, &self.options)?;
        self.append(lease, stamp, events, observed).await
    }
    /// Persist an explicit gap without choosing or acknowledging recovery.
    pub async fn store_gap(
        &self,
        lease: &SourceLease,
        gap: &tg_mtproto::ResyncRequired,
    ) -> Result<Receipt> {
        if project::identity(gap.identity())? != self.account {
            return Err(invalid());
        }
        let observed = Instant::now();
        let (stamp, events) = project::gap(gap, &self.options)?;
        self.append(lease, stamp, events, observed).await
    }
    /// One bounded receive stage, durable append, then exact whole-batch ACK.
    /// Cancellation can finish owned work; repeat through the same feed/receiver.
    pub async fn ingest(
        &self,
        lease: &mut SourceLease,
        receiver: &mut tg_mtproto::UpdateReceiver,
        wait: Duration,
    ) -> std::result::Result<Ingested, IngestError> {
        if project::identity(receiver.identity())? != self.account
            || receiver.request_budget() > self.journal.lease_duration() / 2
        {
            return Err(invalid().into());
        }
        self.check_scope(lease)?;
        self.check_source(lease, receiver.source_id()).await?;
        *lease = self.renew_source(lease).await?;
        let observed = Instant::now();
        let Some(item) = receiver.next_batch(wait).await? else {
            return Ok(Ingested::Idle);
        };
        match item {
            tg_mtproto::UpdateItem::Batch(batch) => {
                if project::identity(batch.identity())? != self.account {
                    return Err(invalid().into());
                }
                let (stamp, events) = project::batch(&batch, &self.options)?;
                let receipt = self.append(lease, stamp, events, observed).await?;
                *lease = self.renew_source(lease).await?;
                receiver.acknowledge(&batch).await?;
                self.confirm(batch.source_id(), batch.id()).await?;
                Ok(Ingested::Batch(receipt))
            }
            tg_mtproto::UpdateItem::Gap(gap) => {
                if project::identity(gap.identity())? != self.account {
                    return Err(invalid().into());
                }
                let (stamp, events) = project::gap(&gap, &self.options)?;
                self.append(lease, stamp, events, observed).await?;
                Ok(Ingested::Gap(gap))
            }
        }
    }
}
