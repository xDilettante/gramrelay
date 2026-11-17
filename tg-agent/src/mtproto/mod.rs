//! Typed MTProto agent contracts. Policy checks are local, not execution permits.
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
//! # Ok(()) }
//! ```
mod grant;
mod types;
pub mod updates;
pub use crate::executor::{
    Approval, Budget, BudgetLimits, MtprotoDryRun as DryRun, PolicyStore, Principal,
};
pub(crate) use grant::OriginalScope;
pub use grant::{Grant, PeerScope};
pub use types::*;
