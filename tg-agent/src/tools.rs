//! Shared machine interface. Discovery metadata never grants execution authority.
mod catalog;
mod codec;
mod request;
mod session;
pub use catalog::{
    ToolAnnotations, ToolCatalog, ToolDefinition, catalog, request_schema, response_schema,
};
#[cfg(feature = "mtproto")]
pub use catalog::{extended_catalog, extended_request_schema, extended_response_schema};
pub use codec::encode_tool_response;
pub use codec::{OutputLimit, encode_response};
pub use request::ToolRequest;
pub use session::{ExecutionMode, ToolError, ToolResponse, ToolSession};
