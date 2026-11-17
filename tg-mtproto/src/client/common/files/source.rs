#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
use super::super::*;
use md5::Md5;
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};
use zeroize::Zeroizing;

pub(super) enum Source {
    Bytes { bytes: bytes::Bytes, retained: u64 },
    Path(PathBuf),
}
impl Source {
    pub fn owner_guard(&self) -> Option<bytes::Bytes> {
        match self {
            Self::Bytes { bytes, .. } => Some(bytes.clone()),
            Self::Path(_) => None,
        }
    }
    pub fn bytes(bytes: Vec<u8>) -> Self {
        let retained = bytes.capacity() as u64;
        Self::Bytes {
            bytes: bytes::Bytes::from_owner(Zeroizing::new(bytes)),
            retained,
        }
    }
    pub fn owned(owner: impl AsRef<[u8]> + Send + 'static) -> Self {
        let bytes = bytes::Bytes::from_owner(owner);
        Self::Bytes {
            retained: bytes.len() as u64,
            bytes,
        }
    }
    pub fn path(path: PathBuf) -> Self {
        Self::Path(path)
    }
}
enum Opened {
    Bytes(bytes::Bytes),
    File(File),
}
pub(super) struct Prepared {
    source: Opened,
    size: u64,
    at: u64,
    part_size: usize,
    first: [u8; 32],
    second: Sha256,
    md5: Zeroizing<String>,
}
pub(super) struct FileStats {
    pub size: u64,
    pub sha256: [u8; 32],
}
fn file_error() -> super::super::super::ClientError {
    error(ClientErrorKind::File, Outcome::NotSent)
}
fn changed(outcome: Outcome) -> super::super::super::ClientError {
    error(ClientErrorKind::Integrity, outcome)
}
impl Prepared {
    pub fn open(
        source: Source,
        part_size: usize,
        max_bytes: u64,
        expected: Option<[u8; 32]>,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        if part_size == 0 || part_size > 512 * 1024 {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let (mut source, size) = match source {
            Source::Bytes { bytes, retained } => {
                if retained > max_bytes {
                    return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
                }
                let size = bytes.len() as u64;
                (Opened::Bytes(bytes), size)
            }
            Source::Path(path) => {
                if !path.is_absolute() {
                    return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
                }
                let file = File::open(path).map_err(|_| file_error())?;
                let metadata = file.metadata().map_err(|_| file_error())?;
                if !metadata.is_file() {
                    return Err(file_error());
                }
                let size = metadata.len();
                (Opened::File(file), size)
            }
        };
        if size == 0 || size > max_bytes {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let mut sha256 = Sha256::new();
        let mut md5 = (size <= 10 * 1024 * 1024).then(Md5::new);
        match &mut source {
            Opened::Bytes(bytes) => {
                for part in bytes.chunks(part_size) {
                    check()?;
                    sha256.update(part);
                    if let Some(md5) = &mut md5 {
                        md5.update(part);
                    }
                }
            }
            Opened::File(file) => {
                let mut buffer = Zeroizing::new(vec![0; part_size]);
                let mut remaining = size;
                while remaining > 0 {
                    check()?;
                    let count = remaining.min(part_size as u64) as usize;
                    file.read_exact(&mut buffer[..count]).map_err(|failure| {
                        if failure.kind() == std::io::ErrorKind::UnexpectedEof {
                            changed(Outcome::NotSent)
                        } else {
                            file_error()
                        }
                    })?;
                    sha256.update(&buffer[..count]);
                    if let Some(md5) = &mut md5 {
                        md5.update(&buffer[..count]);
                    }
                    remaining -= count as u64;
                }
                eof(file, Outcome::NotSent, check)?;
                file.seek(SeekFrom::Start(0)).map_err(|_| file_error())?;
            }
        }
        check()?;
        let first: [u8; 32] = sha256.finalize().into();
        if expected.is_some_and(|expected| expected != first) {
            return Err(changed(Outcome::NotSent));
        }
        let md5 = md5
            .map(|md5| {
                let mut text = String::with_capacity(32);
                for byte in md5.finalize() {
                    write!(text, "{byte:02x}").expect("String formatting is infallible");
                }
                text
            })
            .unwrap_or_default();
        Ok(Self {
            source,
            size,
            at: 0,
            part_size,
            first,
            second: Sha256::new(),
            md5: Zeroizing::new(md5),
        })
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn md5_checksum(&self) -> &str {
        &self.md5
    }
    pub fn has_remaining(&self) -> bool {
        self.at < self.size
    }
    #[cfg(test)]
    fn bytes_pointer(&self) -> Option<*const u8> {
        match &self.source {
            Opened::Bytes(bytes) => Some(bytes.as_ptr()),
            _ => None,
        }
    }
    pub fn read_next(
        mut self,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<(Self, Zeroizing<Vec<u8>>)> {
        check()?;
        if self.at >= self.size {
            return Err(changed(Outcome::Unknown));
        }
        let count = (self.size - self.at).min(self.part_size as u64) as usize;
        let mut bytes = Zeroizing::new(vec![0; count]);
        match &mut self.source {
            Opened::Bytes(source) => {
                let start = usize::try_from(self.at).map_err(|_| changed(Outcome::Unknown))?;
                bytes.copy_from_slice(
                    source
                        .get(start..start + count)
                        .ok_or_else(|| changed(Outcome::Unknown))?,
                );
            }
            Opened::File(file) => file
                .read_exact(&mut bytes)
                .map_err(|_| changed(Outcome::Unknown))?,
        }
        self.second.update(&*bytes);
        self.at += count as u64;
        check()?;
        Ok((self, bytes))
    }
    pub fn finish(mut self, check: &dyn Fn() -> Result<()>) -> Result<FileStats> {
        check()?;
        if self.at != self.size {
            return Err(changed(Outcome::Unknown));
        }
        if let Opened::File(file) = &mut self.source {
            eof(file, Outcome::Unknown, check)?;
        }
        let second: [u8; 32] = self.second.finalize().into();
        if second != self.first {
            return Err(changed(Outcome::Unknown));
        }
        check()?;
        Ok(FileStats {
            size: self.size,
            sha256: self.first,
        })
    }
}
fn eof(file: &mut File, outcome: Outcome, check: &dyn Fn() -> Result<()>) -> Result<()> {
    let mut extra = Zeroizing::new([0; 1]);
    loop {
        check()?;
        match file.read(&mut *extra) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(changed(outcome)),
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(changed(outcome)),
        }
    }
}
