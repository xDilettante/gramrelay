use crate::journal::{
    JournalError, JournalErrorKind, JournalOptions, Result,
    types::{millis, valid_id},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr, time::Duration};

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub(super) retention: Duration,
    pub(super) max_event_bytes: usize,
    pub(super) max_batch_bytes: usize,
}
impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            retention: Duration::from_secs(7 * 24 * 3600),
            max_event_bytes: 1024 * 1024,
            max_batch_bytes: 1024 * 1024,
        }
    }
}
impl UpdateOptions {
    pub fn retention(mut self, value: Duration) -> Self {
        self.retention = value;
        self
    }
    pub fn max_event_bytes(mut self, value: usize) -> Self {
        self.max_event_bytes = value;
        self
    }
    pub fn max_batch_bytes(mut self, value: usize) -> Self {
        self.max_batch_bytes = value;
        self
    }
    pub(super) fn validate(&self, limits: &JournalOptions) -> Result<()> {
        millis(self.retention)?;
        if self.max_event_bytes == 0
            || self.max_event_bytes > self.max_batch_bytes
            || self.max_batch_bytes > limits.max_update_bytes
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn invalid() -> JournalError {
    JournalError::new(JournalErrorKind::Validation)
}

/// An opaque position, not an access grant. Always check the caller's current scopes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UpdateCursor {
    pub(super) profile: String,
    pub(super) sequence: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorWire {
    profile: String,
    sequence: i64,
}
impl From<UpdateCursor> for String {
    fn from(value: UpdateCursor) -> Self {
        let bytes = serde_json::to_vec(&CursorWire {
            profile: value.profile,
            sequence: value.sequence,
        })
        .expect("strings and integer serialize");
        format!("tgu1.{}", URL_SAFE_NO_PAD.encode(bytes))
    }
}
impl FromStr for UpdateCursor {
    type Err = JournalError;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() > 512 {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(value.strip_prefix("tgu1.").ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let wire: CursorWire = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        valid_id(&wire.profile)?;
        if wire.sequence < 0 {
            return Err(invalid());
        }
        Ok(Self {
            profile: wire.profile,
            sequence: wire.sequence,
        })
    }
}
impl TryFrom<String> for UpdateCursor {
    type Error = JournalError;
    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}
impl fmt::Display for UpdateCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from(self.clone()))
    }
}
impl fmt::Debug for UpdateCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateCursor").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    Polling,
    Webhook,
}

/// Private proof of the checkpoint read for one source generation. Use immediately.
#[derive(Clone)]
pub struct PollCheckpoint {
    pub(super) profile: String,
    pub(super) bot_id: i64,
    pub(super) source_revision: i64,
    pub(super) revision: i64,
    pub(super) offset: Option<i64>,
    pub(super) issuer: String,
    pub(super) until_ms: i64,
}
impl PollCheckpoint {
    pub fn offset(&self) -> Option<i64> {
        self.offset
    }
}
impl fmt::Debug for PollCheckpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollCheckpoint").finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub struct PollReceipt {
    pub(super) inserted: usize,
    pub(super) duplicates: usize,
    pub(super) next_offset: Option<i64>,
}
impl PollReceipt {
    pub fn inserted(&self) -> usize {
        self.inserted
    }
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }
    pub fn next_offset(&self) -> Option<i64> {
        self.next_offset
    }
}
impl SourceMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Polling => "polling",
            Self::Webhook => "webhook",
        }
    }
}
#[derive(Clone)]
pub struct SourceLease {
    pub(super) issuer: String,
    pub(super) profile: String,
    pub(super) bot_id: i64,
    pub(super) revision: i64,
    pub(super) mode: SourceMode,
    pub(super) until_ms: i64,
}
impl SourceLease {
    pub fn mode(&self) -> SourceMode {
        self.mode
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.until_ms
    }
}
impl fmt::Debug for SourceLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceLease")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct AppendReceipt {
    pub(super) inserted: bool,
    pub(super) cursor: UpdateCursor,
}
impl AppendReceipt {
    pub fn inserted(&self) -> bool {
        self.inserted
    }
    pub fn cursor(&self) -> &UpdateCursor {
        &self.cursor
    }
}

#[derive(Clone, Serialize)]
pub struct StoredUpdate {
    pub(super) cursor: UpdateCursor,
    pub(super) update: tg::Update,
}
impl StoredUpdate {
    pub fn cursor(&self) -> &UpdateCursor {
        &self.cursor
    }
    pub fn update(&self) -> &tg::Update {
        &self.update
    }
    pub fn into_update(self) -> tg::Update {
        self.update
    }
}
impl fmt::Debug for StoredUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredUpdate").finish_non_exhaustive()
    }
}

#[derive(Serialize)]
pub struct UpdatePage {
    pub(super) events: Vec<StoredUpdate>,
    pub(super) next_cursor: UpdateCursor,
    pub(super) has_more: bool,
}

#[derive(Clone)]
pub struct ConsumerLease {
    pub(super) profile: String,
    pub(super) bot_id: i64,
    pub(super) consumer_id: String,
    pub(super) revision: i64,
    pub(super) issuer: String,
    pub(super) until_ms: i64,
}
impl ConsumerLease {
    pub fn expires_at_ms(&self) -> i64 {
        self.until_ms
    }
}
impl fmt::Debug for ConsumerLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsumerLease").finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct ConsumerAck {
    pub(super) lease: ConsumerLease,
    pub(super) start: i64,
    pub(super) end: i64,
}
impl fmt::Debug for ConsumerAck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsumerAck").finish_non_exhaustive()
    }
}
pub struct ConsumerBatch {
    pub(super) page: UpdatePage,
    pub(super) acknowledgement: ConsumerAck,
}
impl ConsumerBatch {
    pub fn events(&self) -> &[StoredUpdate] {
        self.page.events()
    }
    pub fn has_more(&self) -> bool {
        self.page.has_more()
    }
    pub fn into_parts(self) -> (Vec<tg::Update>, ConsumerAck) {
        (
            self.page
                .events
                .into_iter()
                .map(StoredUpdate::into_update)
                .collect(),
            self.acknowledgement,
        )
    }
}
impl fmt::Debug for ConsumerBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsumerBatch")
            .field("count", &self.page.events.len())
            .finish_non_exhaustive()
    }
}
impl UpdatePage {
    pub fn events(&self) -> &[StoredUpdate] {
        &self.events
    }
    pub fn next_cursor(&self) -> &UpdateCursor {
        &self.next_cursor
    }
    pub fn has_more(&self) -> bool {
        self.has_more
    }
}
impl fmt::Debug for UpdatePage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdatePage")
            .field("count", &self.events.len())
            .field("has_more", &self.has_more)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_codec_is_bounded_versioned_and_lossless_for_large_positions() {
        let cursor = UpdateCursor {
            profile: "PRIVATE_PROFILE / бот".into(),
            sequence: i64::MAX,
        };
        assert_eq!(cursor.to_string().parse::<UpdateCursor>().unwrap(), cursor);
        assert!(!format!("{cursor:?}").contains("PRIVATE_"));
        for wire in [
            CursorWire {
                profile: "bot".into(),
                sequence: -1,
            },
            CursorWire {
                profile: String::new(),
                sequence: 0,
            },
        ] {
            let value = format!(
                "tgu1.{}",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&wire).unwrap())
            );
            assert!(value.parse::<UpdateCursor>().is_err());
        }
        assert!(
            cursor
                .to_string()
                .replacen("tgu1.", "tgu99.", 1)
                .parse::<UpdateCursor>()
                .is_err()
        );
    }
}
