#![doc = include_str!("../README.md")]
mod client;
mod session;
pub use client::auth;
pub use client::{
    AppCredentials, BotToken, Client, ClientError, ClientErrorKind, ClientOptions, Outcome,
};
pub use grammers_tl_types as tl;
pub use session::{
    AccountId, AccountIdentity, AccountKind, Session, SessionBinding, SessionError,
    SessionErrorKind, SessionKey, SessionOptions,
};
