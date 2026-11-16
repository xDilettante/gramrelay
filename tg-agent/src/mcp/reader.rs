// Inspect one bounded raw frame before the SDK turns arguments into a Map.
// Map normalization would otherwise silently erase duplicate object members.
// JSON-RPC parsing, lifecycle and dispatch remain entirely the SDK's job.
use super::{MAX_FRAME_BYTES, transport::Boundary};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::{
    cell::Cell,
    collections::HashSet,
    fmt, io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncBufRead, AsyncRead, BufReader, ReadBuf};

pub(super) struct CheckedReader<R> {
    inner: BufReader<R>,
    frame: Vec<u8>,
    offset: usize,
    ready: bool,
    boundary: Arc<Boundary>,
}
impl<R: AsyncRead> CheckedReader<R> {
    pub fn new(inner: R, boundary: Arc<Boundary>) -> Self {
        Self {
            inner: BufReader::with_capacity(8192, inner),
            frame: Vec::new(),
            offset: 0,
            ready: false,
            boundary,
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for CheckedReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if this.ready {
                let count = output.remaining().min(this.frame.len() - this.offset);
                output.put_slice(&this.frame[this.offset..this.offset + count]);
                this.offset += count;
                if this.offset == this.frame.len() {
                    this.ready = false;
                    this.offset = 0;
                    this.frame.clear();
                }
                return Poll::Ready(Ok(()));
            }
            let bytes = match Pin::new(&mut this.inner).poll_fill_buf(cx) {
                Poll::Ready(Ok(bytes)) => bytes,
                Poll::Ready(Err(_)) => {
                    this.boundary.fail();
                    return Poll::Ready(Err(io::Error::other("MCP input failed")));
                }
                Poll::Pending => return Poll::Pending,
            };
            if bytes.is_empty() {
                this.frame.clear();
                return Poll::Ready(Ok(()));
            }
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(bytes.len(), |index| index + 1);
            if this.frame.len().saturating_add(count) > MAX_FRAME_BYTES {
                this.boundary.fail();
                return Poll::Ready(Err(io::Error::other("MCP input limit")));
            }
            this.frame.extend_from_slice(&bytes[..count]);
            Pin::new(&mut this.inner).consume(count);
            if newline.is_some() {
                let duplicate = Cell::new(false);
                // Match the SDK's UTF-8 BOM compatibility before scanning.
                let json = this
                    .frame
                    .strip_prefix(&[0xef, 0xbb, 0xbf])
                    .unwrap_or(&this.frame);
                let _ =
                    Unique(&duplicate).deserialize(&mut serde_json::Deserializer::from_slice(json));
                if duplicate.get() {
                    this.boundary.fail();
                    return Poll::Ready(Err(io::Error::other("MCP duplicate JSON member")));
                }
                this.ready = true;
            }
        }
    }
}
#[derive(Clone, Copy)]
struct Unique<'a>(&'a Cell<bool>);
impl<'de> DeserializeSeed<'de> for Unique<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Unique<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON with unique object members")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                self.0.set(true);
                return Err(serde::de::Error::custom("duplicate JSON member"));
            }
            map.next_value_seed(self)?;
        }
        Ok(())
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        while seq.next_element_seed(self)?.is_some() {}
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}
