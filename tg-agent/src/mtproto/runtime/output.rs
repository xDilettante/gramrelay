use super::*;
use crate::{
    executor::MessageId,
    journal::{Operation, OperationState, ResultAvailability},
};
use serde::{Deserialize, Serialize};
#[derive(Serialize)]
pub struct AccountInfo {
    kind: &'static str,
    account_id: String,
}
impl AccountInfo {
    pub fn kind(&self) -> &str {
        self.kind
    }
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
}
#[derive(Serialize)]
pub struct PeerInfo {
    peer: PeerId,
    cached: bool,
}
impl PeerInfo {
    pub fn peer(&self) -> PeerId {
        self.peer
    }
    pub fn is_cached(&self) -> bool {
        self.cached
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReceipt {
    pub(super) peer: PeerId,
    pub(super) message_id: MessageId,
}
impl MessageReceipt {
    pub fn peer(&self) -> PeerId {
        self.peer
    }
    pub fn message_id(&self) -> MessageId {
        self.message_id
    }
}
#[derive(Serialize)]
pub struct FailureReceipt {
    kind: &'static str,
    outcome: &'static str,
    rpc_code: Option<i32>,
    retry_after_seconds: Option<u32>,
}
impl FailureReceipt {
    pub fn kind(&self) -> &str {
        self.kind
    }
    pub fn outcome(&self) -> &str {
        self.outcome
    }
}
#[derive(Serialize)]
pub struct OperationReceipt {
    operation_id: String,
    action: Action,
    state: OperationState,
    result_availability: ResultAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<MessageReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<FailureReceipt>,
}
impl OperationReceipt {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn action(&self) -> Action {
        self.action
    }
    pub fn state(&self) -> OperationState {
        self.state
    }
    pub fn result_availability(&self) -> ResultAvailability {
        self.result_availability
    }
    pub fn message(&self) -> Option<&MessageReceipt> {
        self.message.as_ref()
    }
    pub fn failure(&self) -> Option<&FailureReceipt> {
        self.failure.as_ref()
    }
}
#[derive(Serialize)]
pub struct ExecutionResult {
    schema_version: u32,
    #[serde(flatten)]
    value: Value,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Value {
    Account { account: AccountInfo },
    Peer { peer: PeerInfo },
    Updates { updates: updates::ReadPage },
    Operation { operation: OperationReceipt },
}
impl ExecutionResult {
    pub(super) fn account(expected: ProfileIdentity) -> Self {
        Self {
            schema_version: 1,
            value: Value::Account {
                account: AccountInfo {
                    kind: match expected.kind() {
                        ProfileKind::MtprotoBot => "mtproto_bot",
                        _ => "mtproto_user",
                    },
                    account_id: expected.account_id().to_string(),
                },
            },
        }
    }
    pub(super) fn peer(peer: PeerId, cached: bool) -> Self {
        Self {
            schema_version: 1,
            value: Value::Peer {
                peer: PeerInfo { peer, cached },
            },
        }
    }
    pub(super) fn updates(updates: updates::ReadPage) -> Self {
        Self {
            schema_version: 1,
            value: Value::Updates { updates },
        }
    }
    pub fn account_info(&self) -> Option<&AccountInfo> {
        if let Value::Account { account } = &self.value {
            Some(account)
        } else {
            None
        }
    }
    pub fn peer_info(&self) -> Option<&PeerInfo> {
        if let Value::Peer { peer } = &self.value {
            Some(peer)
        } else {
            None
        }
    }
    pub fn read_page(&self) -> Option<&updates::ReadPage> {
        if let Value::Updates { updates } = &self.value {
            Some(updates)
        } else {
            None
        }
    }
    pub fn operation(&self) -> Option<&OperationReceipt> {
        if let Value::Operation { operation } = &self.value {
            Some(operation)
        } else {
            None
        }
    }
}
pub(super) fn project(operation: Operation, scope: &Scope) -> Result<ExecutionResult> {
    let message = operation
        .result()
        .map(|value| {
            serde_json::from_value::<MessageReceipt>(value.clone()).map_err(|_| uncertain())
        })
        .transpose()?;
    if message
        .as_ref()
        .is_some_and(|v| Some(v.peer) != scope.peer || i32::try_from(v.message_id.get()).is_err())
    {
        return Err(uncertain());
    }
    let failure = operation.failure().map(|f| FailureReceipt {
        kind: match f.kind() {
            "executor" => "executor",
            "mtproto_timeout" => "mtproto_timeout",
            "mtproto_rpc" => "mtproto_rpc",
            "mtproto_flood_wait" => "mtproto_flood_wait",
            "mtproto_capacity" => "mtproto_capacity",
            "mtproto_identity_mismatch" => "mtproto_identity_mismatch",
            "mtproto_session" => "mtproto_session",
            "mtproto_decode" => "mtproto_decode",
            "mtproto_peer_unavailable" => "mtproto_peer_unavailable",
            "mtproto_invalid_peer" => "mtproto_invalid_peer",
            "mtproto_closed" => "mtproto_closed",
            "mtproto_backend" => "mtproto_backend",
            _ => "other",
        },
        outcome: match f.outcome() {
            "not_sent" => "not_sent",
            "rejected" => "rejected",
            _ => "unknown",
        },
        rpc_code: f.api_error_code().and_then(|v| i32::try_from(v).ok()),
        retry_after_seconds: f
            .retry_after()
            .and_then(|v| u32::try_from(v.as_secs()).ok()),
    });
    Ok(ExecutionResult {
        schema_version: 1,
        value: Value::Operation {
            operation: OperationReceipt {
                operation_id: operation.operation_id().into(),
                action: scope.action,
                state: operation.state(),
                result_availability: operation.result_availability(),
                message,
                failure,
            },
        },
    })
}
macro_rules! redacted {($($ty:ty),+)=>{$(impl std::fmt::Debug for $ty {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str(concat!(stringify!($ty),"([REDACTED])"))}})+};}
redacted!(
    AccountInfo,
    PeerInfo,
    MessageReceipt,
    OperationReceipt,
    FailureReceipt,
    ExecutionResult
);
