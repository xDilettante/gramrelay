use grammers_mtsender::InvocationError;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Outcome {
    NotSent,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClientErrorKind {
    InvalidInput,
    Capacity,
    Closed,
    Runtime,
    Session,
    Timeout,
    Backend,
    Rpc,
    FloodWait,
    Decode,
    IdentityMismatch,
}

/// Static diagnostic metadata. No upstream error or remote description is retained.
#[derive(Clone, Copy, Debug)]
pub struct ClientError {
    kind: ClientErrorKind,
    outcome: Outcome,
    rpc_code: Option<i32>,
    retry_after_seconds: Option<u32>,
}
impl ClientError {
    pub fn kind(&self) -> ClientErrorKind {
        self.kind
    }
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
    pub fn rpc_code(&self) -> Option<i32> {
        self.rpc_code
    }
    pub fn retry_after_seconds(&self) -> Option<u32> {
        self.retry_after_seconds
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            ClientErrorKind::InvalidInput => "mtproto_invalid_input",
            ClientErrorKind::Capacity => "mtproto_capacity",
            ClientErrorKind::Closed => "mtproto_closed",
            ClientErrorKind::Runtime => "mtproto_runtime",
            ClientErrorKind::Session => "mtproto_session",
            ClientErrorKind::Timeout => "mtproto_timeout",
            ClientErrorKind::Backend => "mtproto_backend",
            ClientErrorKind::Rpc => "mtproto_rpc",
            ClientErrorKind::FloodWait => "mtproto_flood_wait",
            ClientErrorKind::Decode => "mtproto_decode",
            ClientErrorKind::IdentityMismatch => "mtproto_identity_mismatch",
        }
    }
}
impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl Error for ClientError {}
pub(super) type Result<T> = std::result::Result<T, ClientError>;
pub(super) fn error(kind: ClientErrorKind, outcome: Outcome) -> ClientError {
    ClientError {
        kind,
        outcome,
        rpc_code: None,
        retry_after_seconds: None,
    }
}
pub(super) fn invocation(failure: InvocationError) -> ClientError {
    match failure {
        InvocationError::InvalidDc => error(ClientErrorKind::InvalidInput, Outcome::NotSent),
        InvocationError::Rpc(rpc) => {
            let rejected = (300..500).contains(&rpc.code);
            let flood = rejected
                && rpc.code == 420
                && matches!(
                    rpc.name.as_str(),
                    "FLOOD_WAIT" | "FLOOD_PREMIUM_WAIT" | "SLOWMODE_WAIT"
                )
                && rpc.value.is_some();
            ClientError {
                kind: if flood {
                    ClientErrorKind::FloodWait
                } else {
                    ClientErrorKind::Rpc
                },
                outcome: if rejected {
                    Outcome::Rejected
                } else {
                    Outcome::Unknown
                },
                rpc_code: (100..600).contains(&rpc.code).then_some(rpc.code),
                retry_after_seconds: if flood { rpc.value } else { None },
            }
        }
        InvocationError::Deserialize(grammers_mtproto::mtp::DeserializeError::LimitExceeded) => {
            error(ClientErrorKind::Capacity, Outcome::Unknown)
        }
        InvocationError::Deserialize(_) => error(ClientErrorKind::Decode, Outcome::Unknown),
        InvocationError::Session(_) => error(ClientErrorKind::Session, Outcome::Unknown),
        _ => error(ClientErrorKind::Backend, Outcome::Unknown),
    }
}
