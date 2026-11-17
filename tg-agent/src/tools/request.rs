use super::ToolError;
use crate::executor::Request;

/// Validated intent for one protocol. It contains no host authority.
#[derive(Clone, Debug)]
pub enum ToolRequest {
    BotApi(Request),
    #[cfg(feature = "mtproto")]
    Mtproto(crate::mtproto::Request),
}
impl ToolRequest {
    pub const MAX_JSON_BYTES: usize = Request::MAX_JSON_BYTES;
    pub fn from_json(input: &[u8]) -> Result<Self, ToolError> {
        if input.len() > Self::MAX_JSON_BYTES {
            return Err(ToolError::invalid_input());
        }
        if let Ok(request) = Request::from_json(input) {
            return Ok(Self::BotApi(request));
        }
        #[cfg(feature = "mtproto")]
        if let Ok(request) = crate::mtproto::Request::from_json(input) {
            return Ok(Self::Mtproto(request));
        }
        Err(ToolError::invalid_input())
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::BotApi(request) => request.action().name(),
            #[cfg(feature = "mtproto")]
            Self::Mtproto(request) => request.action().name(),
        }
    }
    pub fn is_write(&self) -> bool {
        match self {
            Self::BotApi(request) => request.action().is_write(),
            #[cfg(feature = "mtproto")]
            Self::Mtproto(request) => request.action().is_write(),
        }
    }
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::BotApi(request) => request.operation_id(),
            #[cfg(feature = "mtproto")]
            Self::Mtproto(request) => request.operation_id(),
        }
    }
    pub(crate) fn recovery_tool(&self) -> &'static str {
        match self {
            Self::BotApi(_) => "get_operation",
            #[cfg(feature = "mtproto")]
            Self::Mtproto(_) => "mtproto_get_operation",
        }
    }
}
impl From<Request> for ToolRequest {
    fn from(request: Request) -> Self {
        Self::BotApi(request)
    }
}
#[cfg(feature = "mtproto")]
impl From<crate::mtproto::Request> for ToolRequest {
    fn from(request: crate::mtproto::Request) -> Self {
        Self::Mtproto(request)
    }
}
