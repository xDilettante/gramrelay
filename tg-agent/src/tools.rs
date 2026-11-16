//! Shared machine interface. Discovery metadata never grants execution authority.
mod catalog;
mod codec;
mod session;
pub use catalog::{
    ToolAnnotations, ToolCatalog, ToolDefinition, catalog, request_schema, response_schema,
};
pub use codec::{OutputLimit, encode_response};
pub use session::{ExecutionMode, ToolError, ToolResponse, ToolSession};
