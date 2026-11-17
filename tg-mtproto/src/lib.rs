#![doc = include_str!("../README.md")]
mod client;
mod session;
pub use client::auth;
pub use client::{
    Account, AppCredentials, BotToken, Client, ClientError, ClientErrorKind, ClientOptions, Dialog,
    DialogContinuation, DialogCursor, DialogFolder, DialogId, DialogPage, DialogPageIssue,
    HistoryCursor, HistoryMessage, HistoryMessageKind, HistoryPage, MessageReceipt, Outcome, Peer,
    PeerId, PeerKind, SendText,
};
pub use grammers_tl_types as tl;
pub use session::{
    AccountId, AccountIdentity, AccountKind, Session, SessionBinding, SessionError,
    SessionErrorKind, SessionKey, SessionOptions,
};
