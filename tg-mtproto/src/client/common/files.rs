mod datacenter;
mod download;
mod source;
use super::*;
pub use download::{DownloadOptions, DownloadReceipt, DownloadedDocument, RemoteDocument};
use futures_util::{StreamExt, stream::FuturesUnordered};
use source::{Prepared, Source};
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

const MAX_LOCAL: u64 = 4 * 1024 * 1024 * 1024;
pub struct SendDocument {
    source: Source,
    name: Zeroizing<String>,
    mime: Zeroizing<String>,
    caption: Zeroizing<String>,
    random_id: i64,
    file_id: i64,
    max_bytes: u64,
    parallelism: usize,
    expected_sha256: Option<[u8; 32]>,
    silent: bool,
}
impl fmt::Debug for SendDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SendDocument([REDACTED])")
    }
}
impl SendDocument {
    pub fn bytes(name: impl Into<String>, mime: impl Into<String>, bytes: Vec<u8>) -> Result<Self> {
        let source = Source::bytes(bytes);
        if matches!(&source, Source::Bytes(bytes) if bytes.is_empty()) {
            return Err(invalid());
        }
        Self::new(source, name, mime)
    }
    /// The host supplies a protected absolute path and a separate display name.
    /// File opening/reads are owned blocking work performed only after admission.
    pub fn path(
        path: impl Into<PathBuf>,
        name: impl Into<String>,
        mime: impl Into<String>,
    ) -> Result<Self> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(invalid());
        }
        Self::new(Source::path(path), name, mime)
    }
    fn new(source: Source, name: impl Into<String>, mime: impl Into<String>) -> Result<Self> {
        let name = Zeroizing::new(name.into());
        let mime = Zeroizing::new(mime.into());
        if name.is_empty()
            || name.len() > 255
            || name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        {
            return Err(invalid());
        }
        let Some((kind, subtype)) = mime.split_once('/') else {
            return Err(invalid());
        };
        if mime.len() > 255
            || kind.is_empty()
            || subtype.is_empty()
            || ![kind, subtype].into_iter().all(|part| {
                part.len() <= 127
                    && part.as_bytes()[0].is_ascii_alphanumeric()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
            })
        {
            return Err(invalid());
        }
        Ok(Self {
            source,
            name,
            mime: Zeroizing::new(mime.to_ascii_lowercase()),
            caption: Zeroizing::new(String::new()),
            random_id: text::new_random_id()?,
            file_id: text::new_random_id()?,
            max_bytes: 64 * 1024 * 1024,
            parallelism: 4,
            expected_sha256: None,
            silent: false,
        })
    }
    pub fn random_id(&self) -> i64 {
        self.random_id
    }
    /// Restore a host intent ID; never use this to replay an Unknown result.
    pub fn with_random_id(mut self, id: i64) -> Result<Self> {
        if id == 0 {
            return Err(invalid());
        }
        self.random_id = id;
        Ok(self)
    }
    pub fn caption(mut self, caption: impl Into<String>) -> Result<Self> {
        let caption = Zeroizing::new(caption.into());
        if caption.len() > 4096 || caption.contains('\0') || caption.encode_utf16().count() > 1024 {
            return Err(invalid());
        }
        self.caption = caption;
        Ok(self)
    }
    pub fn max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }
    pub fn parallelism(mut self, parts: usize) -> Self {
        self.parallelism = parts;
        self
    }
    pub fn expected_sha256(mut self, digest: [u8; 32]) -> Self {
        self.expected_sha256 = Some(digest);
        self
    }
    pub fn silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }
    fn validate(&self) -> Result<()> {
        if self.max_bytes == 0 || self.max_bytes > MAX_LOCAL || !(1..=8).contains(&self.parallelism)
        {
            return Err(invalid());
        }
        if let Source::Bytes(bytes) = &self.source
            && (bytes.len() as u64 > self.max_bytes || bytes.capacity() as u64 > self.max_bytes)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn invalid() -> super::super::ClientError {
    error(ClientErrorKind::InvalidInput, Outcome::NotSent)
}

#[derive(Clone, Copy)]
pub struct DocumentReceipt {
    message: MessageReceipt,
    size: u64,
    sha256: [u8; 32],
}
impl fmt::Debug for DocumentReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentReceipt")
            .field("message", &self.message)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}
impl DocumentReceipt {
    pub fn message(&self) -> MessageReceipt {
        self.message
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}
fn part_size(budget: usize) -> Result<usize> {
    let available = budget.saturating_sub(32).min(512 * 1024);
    if available < 1024 {
        return Err(invalid());
    }
    let mut size = 1024;
    while size * 2 <= available {
        size *= 2;
    }
    Ok(size)
}
impl Account {
    /// Upload and send one immutable document intent. No part or final send is
    /// replayed, and ordinary errors drain already-admitted parts first.
    pub async fn send_document(
        &self,
        peer: &Peer,
        intent: SendDocument,
    ) -> Result<DocumentReceipt> {
        intent.validate()?;
        let byte_limit = self.client.inner.options.max_request_bytes;
        let part_size = part_size(byte_limit)?;
        let references = self.references();
        peer.check(&references)?;
        let peer = peer.clone();
        self.run(move |context, session| async move {
            peer.check(&references)?;
            let input_peer = input_peer(&session, peer.id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
            let SendDocument {
                source,
                name,
                mime,
                caption,
                random_id,
                file_id,
                max_bytes,
                parallelism,
                expected_sha256,
                silent,
            } = intent;
            let guard = context.clone();
            let prepared = context
                .compute(move || {
                    Prepared::open(source, part_size, max_bytes, expected_sha256, &|| {
                        guard.check_active()
                    })
                })
                .await?;
            let size = prepared.size();
            let total = i32::try_from(size.div_ceil(part_size as u64)).map_err(|_| invalid())?;
            let big = size > 10 * 1024 * 1024;
            let mut final_bytes = {
                let file = if big {
                    tl::enums::InputFile::Big(tl::types::InputFileBig {
                        id: file_id,
                        parts: total,
                        name: name.to_string(),
                    })
                } else {
                    tl::enums::InputFile::File(tl::types::InputFile {
                        id: file_id,
                        parts: total,
                        name: name.to_string(),
                        md5_checksum: prepared.md5_checksum().to_owned(),
                    })
                };
                let request = MediaRequest(tl::functions::messages::SendMedia {
                    silent,
                    background: false,
                    clear_draft: false,
                    noforwards: false,
                    update_stickersets_order: false,
                    invert_media: false,
                    allow_paid_floodskip: false,
                    peer: input_peer,
                    reply_to: None,
                    media: tl::enums::InputMedia::UploadedDocument(
                        tl::types::InputMediaUploadedDocument {
                            nosound_video: false,
                            force_file: true,
                            spoiler: false,
                            file,
                            thumb: None,
                            mime_type: mime.to_string(),
                            attributes: vec![tl::enums::DocumentAttribute::Filename(
                                tl::types::DocumentAttributeFilename {
                                    file_name: name.to_string(),
                                },
                            )],
                            stickers: None,
                            video_cover: None,
                            video_timestamp: None,
                            ttl_seconds: None,
                        },
                    ),
                    message: caption.to_string(),
                    random_id,
                    reply_markup: None,
                    entities: None,
                    schedule_date: None,
                    schedule_repeat_period: None,
                    send_as: None,
                    quick_reply_shortcut: None,
                    effect: None,
                    allow_paid_stars: None,
                    suggested_post: None,
                });
                super::super::encode_request(&request.0, byte_limit)?
            };
            let mut prepared = Some(prepared);
            let mut pending = FuturesUnordered::new();
            let mut failure = None;
            let mut index = 0;
            while prepared.as_ref().is_some_and(Prepared::has_remaining) {
                if pending.len() == parallelism
                    && let Some(Err(error)) = pending.next().await
                {
                    failure = Some(error);
                    break;
                }
                let source = prepared.take().ok_or_else(malformed)?;
                let guard = context.clone();
                match context
                    .compute(move || source.read_next(&|| guard.check_active()))
                    .await
                {
                    Ok((next, bytes)) => {
                        prepared = Some(next);
                        pending.push(save_part(
                            context.clone(),
                            file_id,
                            index,
                            total,
                            big,
                            bytes,
                            byte_limit,
                        ));
                        index += 1;
                    }
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            while let Some(result) = pending.next().await {
                if let Err(error) = result {
                    failure.get_or_insert(error);
                }
            }
            if let Some(failure) = failure {
                return Err(failure);
            }
            let source = prepared.take().ok_or_else(malformed)?;
            let guard = context.clone();
            let stats = context
                .compute(move || source.finish(&|| guard.check_active()))
                .await?;
            let response = context
                .invoke::<tl::functions::messages::SendMedia>(std::mem::take(&mut *final_bytes))
                .await?;
            let message = match response {
                tl::enums::Updates::UpdateShortSentMessage(sent) => {
                    document_matches(sent.media.as_ref(), size, &name, &mime)?;
                    text::short_receipt(peer.id, &sent)?
                }
                response => {
                    let updates = text::unpack(response, &session, references.expected).await?;
                    let id = text::sent_id(&updates, random_id)?;
                    let receipt = text::matching_message(&updates, peer.id, id, &caption, false)?;
                    let media = updates
                        .iter()
                        .find_map(|update| {
                            let raw = match update {
                                tl::enums::Update::NewMessage(raw) => &raw.message,
                                tl::enums::Update::NewChannelMessage(raw) => &raw.message,
                                _ => return None,
                            };
                            let tl::enums::Message::Message(raw) = raw else {
                                return None;
                            };
                            (raw.id == id && PeerId::from_tl(&raw.peer_id).ok() == Some(peer.id))
                                .then_some(raw.media.as_ref())
                        })
                        .flatten();
                    document_matches(media, size, &name, &mime)?;
                    receipt
                }
            };
            Ok(DocumentReceipt {
                message,
                size: stats.size,
                sha256: stats.sha256,
            })
        })
        .await
    }
}
struct MediaRequest(tl::functions::messages::SendMedia);
impl Drop for MediaRequest {
    fn drop(&mut self) {
        self.0.message.zeroize();
        if let tl::enums::InputMedia::UploadedDocument(media) = &mut self.0.media {
            media.mime_type.zeroize();
            match &mut media.file {
                tl::enums::InputFile::File(file) => {
                    file.name.zeroize();
                    file.md5_checksum.zeroize();
                }
                tl::enums::InputFile::Big(file) => file.name.zeroize(),
                _ => {}
            }
            for attribute in &mut media.attributes {
                if let tl::enums::DocumentAttribute::Filename(value) = attribute {
                    value.file_name.zeroize();
                }
            }
        }
    }
}
enum PartRequest {
    Small(tl::functions::upload::SaveFilePart),
    Big(tl::functions::upload::SaveBigFilePart),
}
impl Drop for PartRequest {
    fn drop(&mut self) {
        match self {
            Self::Small(part) => part.bytes.zeroize(),
            Self::Big(part) => part.bytes.zeroize(),
        }
    }
}
async fn save_part(
    context: Context,
    file_id: i64,
    index: i32,
    total: i32,
    big: bool,
    mut bytes: Zeroizing<Vec<u8>>,
    limit: usize,
) -> Result<()> {
    let mut body = {
        let request = if big {
            PartRequest::Big(tl::functions::upload::SaveBigFilePart {
                file_id,
                file_part: index,
                file_total_parts: total,
                bytes: std::mem::take(&mut *bytes),
            })
        } else {
            PartRequest::Small(tl::functions::upload::SaveFilePart {
                file_id,
                file_part: index,
                bytes: std::mem::take(&mut *bytes),
            })
        };
        match &request {
            PartRequest::Small(request) => super::super::encode_request(request, limit)?,
            PartRequest::Big(request) => super::super::encode_request(request, limit)?,
        }
    };
    let saved = if big {
        context
            .invoke::<tl::functions::upload::SaveBigFilePart>(std::mem::take(&mut *body))
            .await?
    } else {
        context
            .invoke::<tl::functions::upload::SaveFilePart>(std::mem::take(&mut *body))
            .await?
    };
    if saved {
        Ok(())
    } else {
        Err(error(ClientErrorKind::Rpc, Outcome::Rejected))
    }
}
fn document_matches(
    media: Option<&tl::enums::MessageMedia>,
    size: u64,
    name: &str,
    mime: &str,
) -> Result<()> {
    let Some(tl::enums::MessageMedia::Document(media)) = media else {
        return Err(malformed());
    };
    let Some(tl::enums::Document::Document(document)) = &media.document else {
        return Err(malformed());
    };
    if document.id == 0
        || document.dc_id <= 0
        || document.date <= 0
        || document.size != size as i64
        || !document.mime_type.eq_ignore_ascii_case(mime)
        || document.attributes.len() > 1024
    {
        return Err(malformed());
    }
    let mut names = document
        .attributes
        .iter()
        .filter_map(|attribute| match attribute {
            tl::enums::DocumentAttribute::Filename(value) => Some(value.file_name.as_str()),
            _ => None,
        });
    if names.next() != Some(name) || names.next().is_some() {
        return Err(malformed());
    }
    Ok(())
}
