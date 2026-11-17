//! Host-authorized, typed agent actions. Tool content never supplies authority.
//!
//! The policy layer is local and performs no I/O. It is not an execution permit:
//! the executor must recheck the current grant when admitting and disclosing work.

mod policy;
mod runtime;
mod types;
#[cfg(feature = "host")]
pub(crate) use runtime::validate_cursor;
#[cfg(feature = "mtproto")]
pub(crate) use types::{map_only, numeric_string, valid_id};
#[cfg(feature = "host")]
pub(crate) use types::{map_only as deserialize_object, valid_id as validate_identifier};

pub use runtime::{
    BotInfo, CallbackClockPolicy, CallbackReference, ChatInfo, ExecutionResult, Executor,
    ExecutorOptions, FailureReceipt, FileLimits, MessageReceipt, OperationReceipt, ReadEvent,
    ReadEventScope, ReadPage, RegisteredFile,
};

#[cfg(feature = "mtproto")]
pub use policy::MtprotoDryRun;
pub use policy::{
    Approval, Budget, BudgetLimits, ChatScope, DryRun, Grant, PolicyStore, Principal,
};
pub use types::{
    Action, ActionCall, AnswerCallbackQueryArgs, ChatId, EditMessageTextArgs, EmptyArgs,
    ExecutorError, ExecutorErrorKind, GetChatArgs, GetOperationArgs, MessageId, ReadUpdatesArgs,
    Request, SendDocumentArgs, SendMessageArgs,
};
