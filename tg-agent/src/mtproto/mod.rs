//! Typed MTProto contracts, durable event reads and host-authorized actions.
//! The Bot API v1 catalog and its ID semantics remain separate.
//!
//! ```
//! use tg_agent::{mtproto::{Action, Budget, Grant, PeerId, PeerKind, PeerScope,
//!     PolicyStore, Principal}, profiles::ProfileIdentity};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let policy = PolicyStore::default();
//! let principal = Principal::new("operator")?;
//! let grant = Grant::new(ProfileIdentity::mtproto_user(42)?, Budget::new("daily")?)?
//!     .allow(Action::SendMessage)
//!     .peers(PeerScope::only([PeerId::new(PeerKind::User, 7)?]));
//! policy.replace_mtproto(&principal, "personal", grant)?;
//! let executor = tg_agent::mtproto::Executor::new(policy,
//!     tg_agent::mtproto::ExecutorOptions::default().max_in_flight(8))?;
//! # let _ = executor;
//! # Ok(()) }
//! ```
mod grant;
mod runtime;
mod types;
pub use runtime::{
    AccountInfo, CallbackClockPolicy, CallbackReference, ExecutionResult, Executor,
    ExecutorOptions, FailureReceipt, FileLimits, MessageReceipt, OperationReceipt, PeerInfo,
    RegisteredFile,
};
pub mod updates;
pub use crate::executor::{
    Approval, Budget, BudgetLimits, MtprotoDryRun as DryRun, PolicyStore, Principal,
};
pub(crate) use grant::OriginalScope;
pub use grant::{Grant, PeerScope};
pub use types::*;
