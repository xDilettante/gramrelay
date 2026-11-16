use super::{ToolError, ToolResponse};
use crate::executor::Request;
use std::io::{self, Write};

/// A validated maximum serialized response size, excluding an NDJSON newline.
#[derive(Debug, Clone, Copy)]
pub struct OutputLimit(usize);
impl Default for OutputLimit {
    fn default() -> Self {
        Self(8 * 1024 * 1024)
    }
}
impl OutputLimit {
    pub fn new(bytes: usize) -> Result<Self, ToolError> {
        if !(1024..=64 * 1024 * 1024).contains(&bytes) {
            return Err(ToolError::new("agent_config", "Invalid output byte limit."));
        }
        Ok(Self(bytes))
    }
    pub fn bytes(self) -> usize {
        self.0
    }
}
pub fn encode_response(
    response: &ToolResponse,
    request: Option<&Request>,
    limit: OutputLimit,
) -> Result<Vec<u8>, ToolError> {
    let mut writer = Buffer {
        bytes: Vec::with_capacity(8192.min(limit.0)),
        limit: limit.0,
    };
    serde_json::to_writer(&mut writer, response)
        .map_err(|_| ToolError::output_too_large(request))?;
    Ok(writer.bytes)
}
struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(data.len())
            .is_none_or(|length| length > self.limit)
        {
            return Err(io::Error::other("response limit"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
