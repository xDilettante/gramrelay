use super::*;
use crate::client::error::RpcClass;
use futures_util::stream::FuturesOrdered;
use sha2::{Digest, Sha256};
use std::io::Write;

struct Key {
    id: i64,
    hash: i64,
    reference: Zeroizing<Vec<u8>>,
    dc: i32,
    size: u64,
    name: Option<Zeroizing<String>>,
    mime: Zeroizing<String>,
}
impl Drop for Key {
    fn drop(&mut self) {
        self.id.zeroize();
        self.hash.zeroize();
    }
}
/// An opaque file reference minted from a verified account/message lookup.
/// It is valid only for the originating client/account/authorization epoch.
#[derive(Clone)]
pub struct RemoteDocument {
    binding: Peer,
    message_id: i32,
    key: Arc<Key>,
}
impl fmt::Debug for RemoteDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteDocument([REDACTED])")
    }
}
impl RemoteDocument {
    pub fn size(&self) -> u64 {
        self.key.size
    }
    pub fn peer(&self) -> PeerId {
        self.binding.id
    }
    pub fn message_id(&self) -> i32 {
        self.message_id
    }
    pub fn name(&self) -> Option<&str> {
        self.key.name.as_deref().map(String::as_str)
    }
    pub fn mime_type(&self) -> &str {
        &self.key.mime
    }
}
#[derive(Clone)]
pub struct DownloadOptions {
    max_bytes: u64,
    parallelism: usize,
    expected: Option<[u8; 32]>,
}
impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            parallelism: 4,
            expected: None,
        }
    }
}
impl fmt::Debug for DownloadOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadOptions")
            .field("max_bytes", &self.max_bytes)
            .field("parallelism", &self.parallelism)
            .finish_non_exhaustive()
    }
}
impl DownloadOptions {
    pub fn max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }
    pub fn parallelism(mut self, count: usize) -> Self {
        self.parallelism = count;
        self
    }
    pub fn expected_sha256(mut self, digest: [u8; 32]) -> Self {
        self.expected = Some(digest);
        self
    }
    fn validate(&self, size: u64) -> Result<()> {
        if self.max_bytes == 0
            || self.max_bytes > MAX_LOCAL
            || size > self.max_bytes
            || !(1..=8).contains(&self.parallelism)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
pub struct DownloadReceipt {
    size: u64,
    sha256: [u8; 32],
}
impl DownloadReceipt {
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}
impl fmt::Debug for DownloadReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadReceipt")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}
pub struct DownloadedDocument {
    bytes: Zeroizing<Vec<u8>>,
    receipt: DownloadReceipt,
}
impl fmt::Debug for DownloadedDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadedDocument")
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}
impl DownloadedDocument {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn receipt(&self) -> DownloadReceipt {
        self.receipt
    }
    /// Transfer content ownership to trusted Rust code (which then owns wiping).
    pub fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut *self.bytes)
    }
}

impl Account {
    /// Resolve only the document belonging to this exact verified peer/message.
    /// Empty/service/text-only messages return None; no raw file key is exposed.
    pub async fn document(&self, peer: &Peer, message_id: i32) -> Result<Option<RemoteDocument>> {
        if message_id <= 0 {
            return Err(invalid());
        }
        let references = self.references();
        peer.check(&references)?;
        let peer = peer.clone();
        let limit = self.client.inner.options.max_request_bytes;
        self.run(move |context, session| async move {
            peer.check(&references)?;
            let input = input_peer(&session, peer.id, references.expected)
                .await?
                .ok_or_else(|| error(ClientErrorKind::PeerUnavailable, Outcome::NotSent))?;
            let ids = vec![tl::enums::InputMessage::Id(tl::types::InputMessageId {
                id: message_id,
            })];
            let response = if let tl::enums::InputPeer::Channel(channel) = input {
                let mut bytes = super::super::super::encode_request(
                    &tl::functions::channels::GetMessages {
                        channel: tl::enums::InputChannel::Channel(tl::types::InputChannel {
                            channel_id: channel.channel_id,
                            access_hash: channel.access_hash,
                        }),
                        id: ids,
                    },
                    limit,
                )?;
                context
                    .invoke::<tl::functions::channels::GetMessages>(std::mem::take(&mut *bytes))
                    .await?
            } else {
                let mut bytes = super::super::super::encode_request(
                    &tl::functions::messages::GetMessages { id: ids },
                    limit,
                )?;
                context
                    .invoke::<tl::functions::messages::GetMessages>(std::mem::take(&mut *bytes))
                    .await?
            };
            let (mut messages, users, chats) = match response {
                tl::enums::messages::Messages::Messages(response) => {
                    (response.messages, response.users, response.chats)
                }
                tl::enums::messages::Messages::Slice(response) => {
                    if response.inexact || response.count < 0 {
                        return Err(malformed());
                    }
                    (response.messages, response.users, response.chats)
                }
                tl::enums::messages::Messages::ChannelMessages(response) => {
                    if peer.id.kind != PeerKind::Channel
                        || response.inexact
                        || response.pts < 0
                        || response.count < 0
                    {
                        return Err(malformed());
                    }
                    (response.messages, response.users, response.chats)
                }
                _ => return Err(malformed()),
            };
            if messages.len() > 1 {
                return Err(malformed());
            }
            let document = if let Some(raw) = messages.pop() {
                let message = HistoryMessage::checked(raw, peer.id)?;
                if message.id() != message_id {
                    return Err(malformed());
                }
                let key = match message.raw() {
                    tl::enums::Message::Message(message) => match &message.media {
                        Some(tl::enums::MessageMedia::Document(media)) => match &media.document {
                            Some(tl::enums::Document::Document(document)) => Some(key(document)?),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                };
                key.map(|key| RemoteDocument {
                    binding: peer,
                    message_id,
                    key: Arc::new(key),
                })
            } else {
                None
            };
            let peers = checked_entities(users, chats, references.expected)?;
            if !peers.is_empty() {
                session
                    .cache_peers(peers)
                    .await
                    .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
            }
            Ok(document)
        })
        .await
    }
    pub async fn download_document(
        &self,
        document: &RemoteDocument,
        options: DownloadOptions,
    ) -> Result<DownloadedDocument> {
        // Large transfers use an owned file destination instead of a huge buffer.
        if document.size() > 64 * 1024 * 1024 {
            return Err(error(ClientErrorKind::Capacity, Outcome::NotSent));
        }
        match self
            .transfer(document, options, Destination::Memory)
            .await?
        {
            Output::Memory(bytes) => Ok(bytes),
            _ => Err(malformed()),
        }
    }
    /// Persist only a fully validated file. Existing destinations are never
    /// replaced. Protected absolute directory/path selection belongs to the host.
    pub async fn download_document_to(
        &self,
        document: &RemoteDocument,
        path: impl Into<PathBuf>,
        options: DownloadOptions,
    ) -> Result<DownloadReceipt> {
        let path = path.into();
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(invalid());
        }
        match self
            .transfer(document, options, Destination::File(path))
            .await?
        {
            Output::File(receipt) => Ok(receipt),
            _ => Err(malformed()),
        }
    }
    async fn transfer(
        &self,
        document: &RemoteDocument,
        options: DownloadOptions,
        destination: Destination,
    ) -> Result<Output> {
        options.validate(document.size())?;
        let references = self.references();
        document.binding.check(&references)?;
        let document = document.clone();
        let gate = self.client.inner.dc_authorization.clone();
        let limit = self.client.inner.options.max_request_bytes;
        let available = self
            .client
            .inner
            .options
            .max_response_bytes
            .saturating_sub(32)
            .min(512 * 1024);
        if available < 4096 {
            return Err(invalid());
        }
        let mut chunk = 4096;
        while chunk * 2 <= available {
            chunk *= 2;
        }
        self.run(move |context, _| async move {
            document.binding.check(&references)?;
            context
                .verify_file_dc(references.expected, document.key.dc, gate.clone(), limit)
                .await?;
            let route = Arc::new(tokio::sync::Mutex::new(Route {
                dc: document.key.dc,
                migrated: false,
                failed: None,
            }));
            let guard = context.clone();
            let size = document.size();
            let mut writer = Some(
                context
                    .compute(move || {
                        Writer::open(destination, size, options.expected, &|| {
                            guard.check_active()
                        })
                    })
                    .await?,
            );
            // A partial last part proves EOF. Aligned sizes need one bounded
            // empty probe, including empty files, to reject an unreported tail.
            let parts = size / chunk as u64 + 1;
            let mut pending = FuturesOrdered::new();
            let mut next = 0;
            let mut failure = None;
            while next < parts && pending.len() < options.parallelism {
                pending.push_back(fetch(
                    context.clone(),
                    document.key.clone(),
                    next * chunk as u64,
                    chunk,
                    route.clone(),
                    gate.clone(),
                    references.expected,
                    limit,
                ));
                next += 1;
            }
            while let Some(result) = pending.next().await {
                let bytes = match result {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                };
                let output = writer.take().ok_or_else(malformed)?;
                let guard = context.clone();
                match context
                    .compute(move || output.accept(bytes, &|| guard.check_active()))
                    .await
                {
                    Ok(output) => writer = Some(output),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
                if next < parts {
                    pending.push_back(fetch(
                        context.clone(),
                        document.key.clone(),
                        next * chunk as u64,
                        chunk,
                        route.clone(),
                        gate.clone(),
                        references.expected,
                        limit,
                    ));
                    next += 1;
                }
            }
            while let Some(result) = pending.next().await {
                if let Err(error) = result {
                    failure.get_or_insert(error);
                }
            }
            if let Some(failure) = failure {
                if let Some(writer) = writer.take() {
                    let _ = context
                        .compute(move || {
                            drop(writer);
                            Ok(())
                        })
                        .await;
                }
                return Err(failure);
            }
            let output = writer.take().ok_or_else(malformed)?;
            let guard = context.clone();
            context
                .compute(move || output.finish(&|| guard.check_active()))
                .await
        })
        .await
    }
}
fn key(document: &tl::types::Document) -> Result<Key> {
    if document.id == 0
        || document.dc_id <= 0
        || document.date <= 0
        || document.size < 0
        || document.file_reference.is_empty()
        || document.file_reference.len() > 16 * 1024
        || document.mime_type.len() > 4096
        || document.attributes.len() > 1024
    {
        return Err(malformed());
    }
    let mut names = document
        .attributes
        .iter()
        .filter_map(|attribute| match attribute {
            tl::enums::DocumentAttribute::Filename(value) => Some(&value.file_name),
            _ => None,
        });
    let name = names.next();
    if names.next().is_some() || name.is_some_and(|name| name.len() > 4096) {
        return Err(malformed());
    }
    Ok(Key {
        id: document.id,
        hash: document.access_hash,
        reference: Zeroizing::new(document.file_reference.clone()),
        dc: document.dc_id,
        size: document.size as u64,
        name: name.map(|name| Zeroizing::new(name.clone())),
        mime: Zeroizing::new(document.mime_type.clone()),
    })
}

struct Route {
    dc: i32,
    migrated: bool,
    failed: Option<super::super::super::ClientError>,
}
struct FileRequest(tl::functions::upload::GetFile);
impl Drop for FileRequest {
    fn drop(&mut self) {
        if let tl::enums::InputFileLocation::InputDocumentFileLocation(location) =
            &mut self.0.location
        {
            location.file_reference.zeroize();
            location.access_hash.zeroize();
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn fetch(
    context: Context,
    key: Arc<Key>,
    offset: u64,
    chunk: usize,
    route: Arc<tokio::sync::Mutex<Route>>,
    gate: Arc<tokio::sync::Mutex<()>>,
    expected: AccountIdentity,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let (mut dc, failed) = {
        let route = route.lock().await;
        (route.dc, route.failed)
    };
    if let Some(failed) = failed {
        return Err(failed);
    }
    let request = FileRequest(tl::functions::upload::GetFile {
        precise: false,
        cdn_supported: false,
        location: tl::enums::InputFileLocation::InputDocumentFileLocation(
            tl::types::InputDocumentFileLocation {
                id: key.id,
                access_hash: key.hash,
                file_reference: key.reference.to_vec(),
                thumb_size: String::new(),
            },
        ),
        offset: offset as i64,
        limit: chunk as i32,
    });
    let body = super::super::super::encode_request(&request.0, limit)?;
    let mut result = context
        .invoke_in_dc::<tl::functions::upload::GetFile>(dc, body.to_vec())
        .await;
    if let Err(failure) = result
        && let RpcClass::FileMigrate(target) = failure.rpc_class()
    {
        let mut state = route.lock().await;
        if let Some(failed) = state.failed {
            return Err(failed);
        }
        if state.dc != dc {
            if state.dc != target {
                state.failed = Some(failure);
                return Err(failure);
            }
            dc = state.dc;
        } else if !state.migrated && dc != target {
            if let Err(error) = context.verify_file_dc(expected, target, gate, limit).await {
                state.failed = Some(error);
                return Err(error);
            }
            state.dc = target;
            state.migrated = true;
            dc = target;
        } else {
            state.failed = Some(failure);
            return Err(failure);
        }
        drop(state);
        result = context
            .invoke_in_dc::<tl::functions::upload::GetFile>(dc, body.to_vec())
            .await;
    }
    let result = match result {
        Ok(tl::enums::upload::File::File(file)) => {
            let bytes = Zeroizing::new(file.bytes);
            let expected = key.size.saturating_sub(offset).min(chunk as u64) as usize;
            if file.mtime < 0 || bytes.len() != expected {
                Err(error(ClientErrorKind::Integrity, Outcome::Unknown))
            } else {
                Ok(bytes)
            }
        }
        Ok(tl::enums::upload::File::CdnRedirect(mut redirect)) => {
            redirect.file_token.zeroize();
            redirect.encryption_key.zeroize();
            redirect.encryption_iv.zeroize();
            Err(error(
                ClientErrorKind::UnsupportedOperation,
                Outcome::Unknown,
            ))
        }
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        route.lock().await.failed.get_or_insert(*error);
    }
    result
}

enum Destination {
    Memory,
    File(PathBuf),
}
enum Storage {
    Memory(Zeroizing<Vec<u8>>),
    File {
        temporary: tempfile::NamedTempFile,
        path: PathBuf,
    },
}
enum Output {
    Memory(DownloadedDocument),
    File(DownloadReceipt),
}
struct Writer {
    storage: Storage,
    expected_size: u64,
    at: u64,
    hash: Sha256,
    expected_hash: Option<[u8; 32]>,
}
impl Writer {
    fn open(
        destination: Destination,
        size: u64,
        expected_hash: Option<[u8; 32]>,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let storage = match destination {
            Destination::Memory => Storage::Memory(Zeroizing::new(Vec::with_capacity(
                usize::try_from(size).map_err(|_| invalid())?,
            ))),
            Destination::File(path) => {
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => return Err(error(ClientErrorKind::File, Outcome::NotSent)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(error(ClientErrorKind::File, Outcome::NotSent)),
                }
                let parent = path.parent().ok_or_else(invalid)?;
                let temporary = tempfile::Builder::new()
                    .prefix(".tg-download-")
                    .tempfile_in(parent)
                    .map_err(|_| error(ClientErrorKind::File, Outcome::NotSent))?;
                Storage::File { temporary, path }
            }
        };
        check()?;
        Ok(Self {
            storage,
            expected_size: size,
            at: 0,
            hash: Sha256::new(),
            expected_hash,
        })
    }
    fn accept(mut self, bytes: Zeroizing<Vec<u8>>, check: &dyn Fn() -> Result<()>) -> Result<Self> {
        check()?;
        self.at = self
            .at
            .checked_add(bytes.len() as u64)
            .filter(|at| *at <= self.expected_size)
            .ok_or_else(|| error(ClientErrorKind::Integrity, Outcome::Unknown))?;
        match &mut self.storage {
            Storage::Memory(output) => output.extend_from_slice(&bytes),
            Storage::File { temporary, .. } => temporary
                .write_all(&bytes)
                .map_err(|_| error(ClientErrorKind::File, Outcome::Unknown))?,
        }
        self.hash.update(&*bytes);
        check()?;
        Ok(self)
    }
    fn finish(self, check: &dyn Fn() -> Result<()>) -> Result<Output> {
        check()?;
        let digest: [u8; 32] = self.hash.finalize().into();
        if self.at != self.expected_size
            || self
                .expected_hash
                .is_some_and(|expected| expected != digest)
        {
            return Err(error(ClientErrorKind::Integrity, Outcome::Unknown));
        }
        let receipt = DownloadReceipt {
            size: self.at,
            sha256: digest,
        };
        match self.storage {
            Storage::Memory(bytes) => {
                check()?;
                Ok(Output::Memory(DownloadedDocument { bytes, receipt }))
            }
            Storage::File { temporary, path } => {
                temporary
                    .as_file()
                    .sync_all()
                    .map_err(|_| error(ClientErrorKind::File, Outcome::Unknown))?;
                check()?;
                let file = temporary
                    .persist_noclobber(path)
                    .map_err(|_| error(ClientErrorKind::File, Outcome::Unknown))?;
                drop(file);
                check()?;
                Ok(Output::File(receipt))
            }
        }
    }
}
