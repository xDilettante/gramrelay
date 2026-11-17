#![doc = include_str!("../README.md")]
mod client;
mod session;
pub use client::auth;
pub use client::{
    Account, AppCredentials, BotToken, Client, ClientError, ClientErrorKind, ClientOptions, Dialog,
    DialogContinuation, DialogCursor, DialogFolder, DialogId, DialogPage, DialogPageIssue,
    DocumentReceipt, DownloadOptions, DownloadReceipt, DownloadedDocument, HistoryCursor,
    HistoryMessage, HistoryMessageKind, HistoryPage, MessageReceipt, Outcome, Peer, PeerId,
    PeerKind, RemoteDocument, SendDocument, SendText,
};
pub use client::{
    ResyncRequired, UpdateBatch, UpdateBatchKind, UpdateGapReason, UpdateItem, UpdateMode,
    UpdateReceiver, UpdateScope,
};
pub use grammers_tl_types as tl;
pub use session::{
    AccountId, AccountIdentity, AccountKind, Session, SessionBinding, SessionError,
    SessionErrorKind, SessionKey, SessionOptions,
};
