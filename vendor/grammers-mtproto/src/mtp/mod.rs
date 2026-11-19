// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation of the [Mobile Transport Protocol].
//!
//! This layer is responsible for converting zero or more input
//! requests into outgoing messages, and to process the response.
//!
//! Separate types are used for [`Plain`] and [`Encrypted`] modes for simplicity,
//! since the `Plain` mode hardly requires to process any state and is only used briefly.
//!
//! [Mobile Transport Protocol]: https://core.telegram.org/mtproto/description

mod encrypted;
mod plain;
mod recovery;

use std::fmt;
use std::time::Instant;

pub use encrypted::{
    ENCRYPTED_PACKET_HEADER_LEN, Encrypted, MAX_TRANSPORT_HEADER_LEN, MESSAGE_CONTAINER_HEADER_LEN,
    PLAIN_PACKET_HEADER_LEN,
};
use grammers_crypto as crypto;
use grammers_tl_types as tl;
pub use plain::Plain;

use crate::{DequeBuffer, MsgId, transport};

/// Manual implementation of Telegram's `rpc_result`.
#[derive(Debug)]
pub struct RpcResult {
    /// Message identifier matching that of the outgoing RPC.
    pub msg_id: MsgId,
    /// Unprocessed response body for the associated RPC.
    pub body: Vec<u8>,
}

/// Wrapper around Telegram's `rpc_error`.
#[derive(Debug)]
pub struct RpcResultError {
    /// Message identifier matching that of the outgoing RPC.
    pub msg_id: MsgId,
    /// Raw RPC error in place of a response.
    pub error: tl::types::RpcError,
}

/// Pseudo-wrapper around Telegram's `bad_msg_notification` constructor.
#[derive(Debug)]
pub struct BadMessage {
    /// Message identifier of the outgoing message that caused this notification.
    pub msg_id: MsgId,
    /// Status code of the bad message notification.
    pub code: i32,
}

/// [`DeserializeError`] with the corresponding [`MsgId`] that caused it.
#[derive(Debug)]
pub struct DeserializationFailure {
    /// Message identifier of the outgoing mesasge whose response failed to be deserialized.
    pub msg_id: MsgId,
    /// Additional details about the deserialization failure.
    pub error: DeserializeError,
}

/// The message whose loss requires recovery.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RecoveryTarget {
    /// A passive server body with no corresponding client request.
    Unsolicited,
    /// The original application request.
    Request(MsgId),
    /// An internal request to refresh the server salt schedule.
    SaltRefresh(MsgId),
}

/// Results from the deserialization of a response.
#[derive(Debug)]
pub enum Deserialization {
    /// Copy of an `rpc_result` that contained an `Updates` type.
    OwnUpdate { msg_id: MsgId, update: Vec<u8> },
    /// `Updates` body received passively when it did not match any `mtproto` constructor.
    Update(Vec<u8>),
    /// `rpc_result` for a previously-sent RPC.
    RpcResult(RpcResult),
    /// `rpc_error` for a previously-sent RPC.
    RpcError(RpcResultError),
    /// `bad_msg_notification` for a previously-sent message.
    BadMessage(BadMessage),
    /// The server recreated an established protocol session. Updates may be missing.
    SessionRecreated,
    /// Recovery of a server body expired or the peer reported that it could not deliver it.
    ResponseUnavailable {
        /// The original request or unsolicited message.
        target: RecoveryTarget,
    },
    /// Generic deserialization failure of a received message.
    Failure(DeserializationFailure),
}

impl BadMessage {
    /// Human-readable description, adapted from
    /// [Service Messages about Messages](https://core.telegram.org/mtproto/service_messages_about_messages).
    pub fn description(&self) -> &'static str {
        match self.code {
            16 => "msg_id too low",
            17 => "msg_id too high",
            18 => "incorrect two lower order msg_id bits; this is a bug",
            19 => {
                "container msg_id is the same as msg_id of a previously received message; this is a bug"
            }
            20 => "message too old",
            32 => "msg_seqno too low",
            33 => "msg_seqno too high",
            34 => "an even msg_seqno expected; this may be a bug",
            35 => "odd msg_seqno expected; this may be a bug",
            48 => "incorrect server salt",
            64 => "invalid container; this is likely a bug",
            _ => "unknown explanation; please report this issue",
        }
    }

    /// Whether sending the message that caused this error can be retried.
    pub fn retryable(&self) -> bool {
        [16, 17, 48].contains(&self.code)
    }

    /// Whether the entire connection should be restarted.
    pub fn fatal(&self) -> bool {
        !self.retryable() && ![32, 33].contains(&self.code)
    }
}

/// A local arithmetic limit that prevents encoding further messages in this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerializeError {
    InvalidClock,
    MessageIdsExhausted,
    SequenceExhausted,
}

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidClock => "MTProto message clock is outside its supported range",
            Self::MessageIdsExhausted => "MTProto message identifiers exhausted",
            Self::SequenceExhausted => "MTProto sequence numbers exhausted",
        })
    }
}

impl std::error::Error for SerializeError {}

/// The error type for the deserialization of server messages.
#[derive(Clone, Debug, PartialEq)]
pub enum DeserializeError {
    /// The server's Authorization Key did not match our expectations.
    BadAuthKey { got: i64, expected: i64 },

    /// The server's message ID did not match our expectations.
    BadMessageId { got: i64 },

    /// The decrypted envelope belongs to a different session.
    UnexpectedSession,

    /// The server's message length was not strictly positive.
    NegativeMessageLength { got: i32 },

    /// The server's message length was past the buffer.
    TooLongMessageLength { got: usize, max_length: usize },

    /// The received buffer is too small to contain a valid response message,
    /// or the response seemed valid at first but trying to deserialize it
    /// proved the buffer to be too small.
    MessageBufferTooSmall,

    /// The server responded with compressed data which we failed to decompress.
    DecompressionFailed,

    /// Incoming encoded data, expansion or nesting exceeded its configured limit.
    LimitExceeded,

    /// A complete TL value was followed by unexpected bytes.
    TrailingData,

    /// A TL field used a noncanonical encoding.
    InvalidEncoding,

    /// The peer used an unsupported protocol control envelope.
    UnsupportedControl,

    /// While deserializing the response types one of them had a constructor
    /// that did not match our expectations. The invalid ID is contained
    /// within this variant.
    UnexpectedConstructor { id: u32 },

    /// Attempting to decrypt the message failed in some way.
    DecryptionError(crypto::Error),

    /// Deserialization failed at the earlier transport level.
    Transport(transport::Error),
}

impl std::error::Error for DeserializeError {}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadAuthKey { got, expected } => {
                write!(f, "bad server auth key (got {got}, expected {expected})")
            }
            Self::BadMessageId { got } => write!(f, "bad server message id (got {got})"),
            Self::UnexpectedSession => write!(f, "unexpected protocol session"),
            Self::NegativeMessageLength { got } => {
                write!(f, "bad server message length (got {got})")
            }
            Self::TooLongMessageLength { got, max_length } => write!(
                f,
                "bad server message length (got {got}, when at most it should be {max_length})"
            ),
            Self::MessageBufferTooSmall => write!(
                f,
                "server responded with a payload that's too small to fit a valid message"
            ),
            Self::DecompressionFailed => write!(f, "failed to decompress server's data"),
            Self::LimitExceeded => write!(f, "incoming payload limit exceeded"),
            Self::TrailingData => write!(f, "unexpected trailing data"),
            Self::InvalidEncoding => write!(f, "invalid TL encoding"),
            Self::UnsupportedControl => write!(f, "unsupported protocol control envelope"),
            Self::UnexpectedConstructor { id } => write!(f, "unexpected constructor: {id:08x}"),
            Self::DecryptionError(ref error) => write!(f, "failed to decrypt message: {error}"),
            Self::Transport(ref error) => {
                write!(f, "deserialization failed at transport level: {error}")
            }
        }
    }
}

impl From<tl::deserialize::Error> for DeserializeError {
    fn from(error: tl::deserialize::Error) -> Self {
        use tl::deserialize::Error as Err;

        match error {
            Err::InvalidEncoding => DeserializeError::InvalidEncoding,
            Err::LimitExceeded => DeserializeError::LimitExceeded,
            Err::UnexpectedEof => DeserializeError::MessageBufferTooSmall,
            Err::UnexpectedConstructor { id } => DeserializeError::UnexpectedConstructor { id },
        }
    }
}

impl From<crypto::Error> for DeserializeError {
    fn from(error: crypto::Error) -> Self {
        Self::DecryptionError(error)
    }
}

/// The trait used by the [Mobile Transport Protocol] to serialize outgoing
/// messages and deserialize incoming ones into proper responses.
///
/// [Mobile Transport Protocol]: https://core.telegram.org/mtproto/description
pub trait Mtp {
    /// Dispatches expired recovery notices and returns the next recovery deadline.
    fn poll_recovery(&mut self, _: Instant, _: &mut Vec<Deserialization>) -> Option<Instant> {
        None
    }

    /// Whether the session must be replaced after an outgoing serialization failure.
    /// A caller must stop using the buffered data when this reports an error.
    fn serialize_error(&self) -> Option<SerializeError> {
        None
    }

    /// Serializes one request to the input buffer.
    /// The same buffer should be used until `finalize` is called.
    ///
    /// Returns the message ID assigned the request if it was serialized, or `None` if the buffer
    /// is full and cannot hold more requests.
    ///
    /// # Panics
    ///
    /// The method panics if the body length is not padded to 4 bytes.
    /// The serialization of requests will always be correctly padded.
    ///
    /// The method also panics if the body length exceeds roughly 1MB.
    /// Such large requests would essentially kill the connection otherwise.
    fn push(&mut self, buffer: &mut DequeBuffer<u8>, request: &[u8]) -> Option<MsgId>;

    /// Finalizes the buffer of requests.
    ///
    /// Note that even if there are no requests to serialize, the protocol may
    /// produce data that has to be sent after deserializing incoming messages.
    ///
    /// The buffer may remain empty if there are no actions to take.
    ///
    /// When at least one message is serialized, the last generated `MsgId` is returned.
    /// This will either belong to the container (if used) or the last serialized message.
    fn finalize(&mut self, buffer: &mut DequeBuffer<u8>) -> Option<MsgId>;

    /// Records the most recently finalized packet after its complete transport write.
    /// [`crate::connection::State`] calls this for each completed packet. Direct
    /// users must call it before decoding responses or starting another packet.
    /// Clearing a finalized buffer without recording the write discards its headers.
    fn on_packet_sent(&mut self) {}

    /// Deserializes a single incoming message payload into zero or more responses.
    fn deserialize(&mut self, payload: &mut [u8])
    -> Result<Vec<Deserialization>, DeserializeError>;
}
