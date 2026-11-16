//! Host-authorized, typed agent actions. Tool content never supplies authority.
//!
//! The policy layer is local and performs no I/O. It is not an execution permit:
//! the executor must recheck the current grant when admitting and disclosing work.

mod policy;
mod runtime;
mod types;

pub use runtime::{
    ExecutionResult, Executor, ExecutorOptions, FailureReceipt, MessageReceipt, OperationReceipt,
};

pub use policy::{
    Approval, Budget, BudgetLimits, ChatScope, DryRun, Grant, PolicyStore, Principal,
};
pub use types::{
    Action, ActionCall, AnswerCallbackQueryArgs, ChatId, EditMessageTextArgs, EmptyArgs,
    ExecutorError, ExecutorErrorKind, GetChatArgs, GetOperationArgs, MessageId, ReadUpdatesArgs,
    Request, SendDocumentArgs, SendMessageArgs,
};
