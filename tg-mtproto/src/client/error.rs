use grammers_mtsender::InvocationError;
use std::{error::Error, fmt};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RpcClass {
    Other,
    Migrate(i32),
    PasswordNeeded,
    CodeInvalid,
    CodeExpired,
    PasswordInvalid,
    SrpExpired,
}

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
    Quarantined,
    InvalidLoginCode,
    LoginCodeExpired,
    InvalidPassword,
    InvalidChallenge,
}

/// Static diagnostic metadata. No upstream error or remote description is retained.
#[derive(Clone, Copy, Debug)]
pub struct ClientError {
    kind: ClientErrorKind,
    outcome: Outcome,
    rpc_code: Option<i32>,
    retry_after_seconds: Option<u32>,
    class: RpcClass,
}
impl ClientError {
    pub(super) fn rpc_class(&self) -> RpcClass {
        self.class
    }
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
            ClientErrorKind::Quarantined => "mtproto_quarantined",
            ClientErrorKind::InvalidLoginCode => "mtproto_invalid_login_code",
            ClientErrorKind::LoginCodeExpired => "mtproto_login_code_expired",
            ClientErrorKind::InvalidPassword => "mtproto_invalid_password",
            ClientErrorKind::InvalidChallenge => "mtproto_invalid_challenge",
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
        class: RpcClass::Other,
    }
}
pub(super) fn invocation(failure: InvocationError) -> ClientError {
    match failure {
        InvocationError::InvalidDc => error(ClientErrorKind::InvalidInput, Outcome::NotSent),
        InvocationError::Rpc(mut rpc) => {
            let rejected = (300..500).contains(&rpc.code);
            let flood = rejected
                && rpc.code == 420
                && matches!(
                    rpc.name.as_str(),
                    "FLOOD_WAIT" | "FLOOD_PREMIUM_WAIT" | "SLOWMODE_WAIT"
                )
                && rpc.value.is_some();
            let class = if rpc.code == 303
                && matches!(
                    rpc.name.as_str(),
                    "PHONE_MIGRATE" | "NETWORK_MIGRATE" | "USER_MIGRATE"
                ) {
                rpc.value
                    .and_then(|value| i32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .map_or(RpcClass::Other, RpcClass::Migrate)
            } else if matches!(rpc.code, 400 | 401) {
                match rpc.name.as_str() {
                    "SESSION_PASSWORD_NEEDED" => RpcClass::PasswordNeeded,
                    "PHONE_CODE_INVALID" | "PHONE_CODE_EMPTY" => RpcClass::CodeInvalid,
                    "PHONE_CODE_EXPIRED" => RpcClass::CodeExpired,
                    "PASSWORD_HASH_INVALID" => RpcClass::PasswordInvalid,
                    "SRP_ID_INVALID" => RpcClass::SrpExpired,
                    _ => RpcClass::Other,
                }
            } else {
                RpcClass::Other
            };
            let result = ClientError {
                kind: if flood {
                    ClientErrorKind::FloodWait
                } else {
                    match class {
                        RpcClass::CodeInvalid => ClientErrorKind::InvalidLoginCode,
                        RpcClass::CodeExpired => ClientErrorKind::LoginCodeExpired,
                        RpcClass::PasswordInvalid => ClientErrorKind::InvalidPassword,
                        _ => ClientErrorKind::Rpc,
                    }
                },
                outcome: if rejected {
                    Outcome::Rejected
                } else {
                    Outcome::Unknown
                },
                rpc_code: (100..600).contains(&rpc.code).then_some(rpc.code),
                retry_after_seconds: if flood { rpc.value } else { None },
                class,
            };
            rpc.name.zeroize();
            result
        }
        InvocationError::Deserialize(grammers_mtproto::mtp::DeserializeError::LimitExceeded) => {
            error(ClientErrorKind::Capacity, Outcome::Unknown)
        }
        InvocationError::Deserialize(_) => error(ClientErrorKind::Decode, Outcome::Unknown),
        InvocationError::Session(_) => error(ClientErrorKind::Session, Outcome::Unknown),
        _ => error(ClientErrorKind::Backend, Outcome::Unknown),
    }
}
