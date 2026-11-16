use crate::{Bot, EffectOutcome, Error, ErrorKind, Result, api::types::File};
use std::fmt;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// A lazy, bounded file download. Paths and credentials are omitted from Debug.
#[must_use = "choose bytes() or to_writer() and await to start the download"]
pub struct Download<'a> {
    bot: &'a Bot,
    path: Option<String>,
    advertised_size: Option<i64>,
    limit: u64,
}
impl fmt::Debug for Download<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download").finish_non_exhaustive()
    }
}
impl Bot {
    pub fn download_file(&self, file: &File) -> Download<'_> {
        Download {
            bot: self,
            path: file.file_path.clone(),
            advertised_size: file.file_size,
            limit: self.download_limit(),
        }
    }
}
impl Download<'_> {
    /// Tighten the trusted Bot-level cap; this cannot increase it.
    pub fn max_bytes(mut self, limit: u64) -> Self {
        self.limit = self.limit.min(limit);
        self
    }

    pub async fn bytes(self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        self.to_writer(&mut output).await?;
        Ok(output)
    }

    /// Stream into a caller-owned writer. Failure/cancellation may leave a prefix.
    /// The caller owns flush, destination creation and overwrite/atomicity decisions.
    pub async fn to_writer<W: AsyncWrite + Unpin + Send>(self, writer: &mut W) -> Result<u64> {
        let limit = self.limit;
        self.bot
            .exchange(
                "download",
                async {
                    if limit == 0 {
                        return Err(Error::local(ErrorKind::Validation, "download_size"));
                    }
                    if let Some(size) = self.advertised_size {
                        if size < 0 {
                            return Err(Error::local(ErrorKind::Validation, "file_size"));
                        }
                        if size as u64 > limit {
                            return Err(Error::local(ErrorKind::Validation, "download_size"));
                        }
                    }
                    let path = self
                        .path
                        .as_deref()
                        .ok_or_else(|| Error::local(ErrorKind::Validation, "file_path"))?;
                    self.bot.file_get(path)
                },
                None,
                |mut response| async move {
                    if !response.status().is_success() {
                        let mut error = Error::new(ErrorKind::Http, EffectOutcome::Unknown);
                        error.http_status = Some(response.status().as_u16());
                        return Err(error);
                    }
                    if response.content_length().is_some_and(|size| size > limit) {
                        return Err(too_large());
                    }
                    let mut written = 0u64;
                    while let Some(chunk) = response
                        .chunk()
                        .await
                        .map_err(crate::client::transport_error)?
                    {
                        if chunk.len() as u64 > limit - written {
                            return Err(too_large());
                        }
                        writer
                            .write_all(&chunk)
                            .await
                            .map_err(|_| Error::new(ErrorKind::Io, EffectOutcome::Unknown))?;
                        written += chunk.len() as u64;
                    }
                    Ok(written)
                },
            )
            .await
    }
}

pub(crate) fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '%' | '?' | '#' | ':'))
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(Error::local(ErrorKind::Validation, "file_path"));
    }
    Ok(())
}
fn too_large() -> Error {
    Error::new(ErrorKind::ResponseTooLarge, EffectOutcome::Unknown)
}
