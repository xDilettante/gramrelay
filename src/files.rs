use crate::{Error, ErrorKind, Result};
use reqwest::{
    Body,
    multipart::{Form, Part},
};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::ReaderStream;

mod reader;
use reader::ReaderUpload;
pub(crate) use reader::UploadReaders;

/// A new upload or a previously returned Telegram file ID.
/// Paths are opened lazily, when the request is awaited.
pub struct InputFile(Source);

enum Source {
    Bytes {
        name: String,
        bytes: Vec<u8>,
    },
    Owned {
        name: String,
        bytes: bytes::Bytes,
    },
    Reader {
        name: String,
        length: u64,
        upload: ReaderUpload,
    },
    Path(PathBuf),
    FileId(String),
}

impl InputFile {
    /// Shares an owned immutable buffer without copying its contents.
    /// The owner stays alive until the multipart body releases its bytes, even
    /// if the caller stops waiting. `AsRef` is evaluated once at construction.
    pub fn owned_bytes(name: impl Into<String>, owner: impl AsRef<[u8]> + Send + 'static) -> Self {
        Self(Source::Owned {
            name: name.into(),
            bytes: bytes::Bytes::from_owner(owner),
        })
    }
    pub fn bytes(name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self(Source::Bytes {
            name: name.into(),
            bytes: bytes.into(),
        })
    }
    /// Streams exactly `length` bytes from an owned reader when the request runs.
    ///
    /// A shorter stream fails during upload. A longer stream's suffix is unread.
    /// The declared length counts toward the upload limit before the first read.
    /// The reader is consumed once and released on completion or cancellation.
    /// An interruption after dispatch can leave an unknown remote effect.
    pub fn reader(
        name: impl Into<String>,
        reader: impl AsyncRead + Send + 'static,
        length: u64,
    ) -> Self {
        Self(Source::Reader {
            name: name.into(),
            length,
            upload: ReaderUpload::new(reader, length),
        })
    }
    pub fn path(path: impl AsRef<Path>) -> Self {
        Self(Source::Path(path.as_ref().to_owned()))
    }
    pub fn file_id(id: impl Into<String>) -> Self {
        Self(Source::FileId(id.into()))
    }

    pub(crate) async fn attach(
        self,
        form: Form,
        max_bytes: u64,
        readers: &mut UploadReaders,
    ) -> Result<Form> {
        if let Source::FileId(id) = self.0 {
            if id.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "file_id"));
            }
            return Ok(form.text("document", id));
        }
        let (part, _) = self.into_part(max_bytes, readers).await?;
        Ok(form.part("document", part))
    }

    pub(crate) async fn into_part(
        self,
        max_bytes: u64,
        readers: &mut UploadReaders,
    ) -> Result<(Part, u64)> {
        let (part, length) = match self.0 {
            Source::FileId(_) => {
                return Err(Error::local(ErrorKind::Validation, "attachment_file"));
            }
            Source::Bytes { name, bytes } => {
                let length = bytes.len() as u64;
                check_size(length, max_bytes)?;
                validate_name(&name)?;
                (Part::bytes(bytes).file_name(name), length)
            }
            Source::Owned { name, bytes } => {
                let length = bytes.len() as u64;
                check_size(length, max_bytes)?;
                validate_name(&name)?;
                (
                    Part::stream_with_length(Body::from(bytes), length).file_name(name),
                    length,
                )
            }
            Source::Reader {
                name,
                length,
                upload,
            } => {
                check_size(length, max_bytes)?;
                validate_name(&name)?;
                let part = upload.part().file_name(name);
                readers.push(upload);
                (part, length)
            }
            Source::Path(path) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| Error::local(ErrorKind::Validation, "file_name"))?;
                validate_name(name)?;
                let file = tokio::fs::File::open(&path)
                    .await
                    .map_err(|_| Error::local(ErrorKind::Io, "file"))?;
                let metadata = file
                    .metadata()
                    .await
                    .map_err(|_| Error::local(ErrorKind::Io, "file"))?;
                if !metadata.is_file() {
                    return Err(Error::local(ErrorKind::Validation, "file"));
                }
                let length = metadata.len();
                check_size(length, max_bytes)?;
                // take(length) also bounds a file that grows after metadata was read.
                // https://docs.rs/tokio/latest/tokio/io/trait.AsyncReadExt.html#method.take
                let stream = ReaderStream::new(file.take(length));
                (
                    Part::stream_with_length(Body::wrap_stream(stream), length)
                        .file_name(name.to_owned()),
                    length,
                )
            }
        };
        let part = part
            .mime_str("application/octet-stream")
            .map_err(|_| Error::local(ErrorKind::Validation, "mime_type"))?;
        Ok((part, length))
    }
}

fn check_size(size: u64, limit: u64) -> Result<()> {
    if size > limit {
        return Err(Error::local(ErrorKind::Validation, "upload_size"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(Error::local(ErrorKind::Validation, "file_name"));
    }
    Ok(())
}
