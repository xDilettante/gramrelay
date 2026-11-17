//! Explicit synthetic client and local endpoint setup. Use synthetic data only.
//! The production constructor never selects this factory from agent input.
use super::*;
pub use grammers_mtsender::InvocationError;

/// Route every configured DC of a synthetic, unattached Session to loopback.
/// This enables observing the real network constructor without external traffic.
/// No connection is started; nonloopback/zero-port/attached inputs refuse.
pub async fn route_loopback(
    session: &Session,
    endpoint: std::net::SocketAddrV4,
) -> std::result::Result<(), crate::SessionError> {
    session.route_fixture_loopback(endpoint).await
}

/// Uses the same owned runner, NoRetries, TL codecs and joined shutdown as M2.
/// Requests include their constructor; responses are boxed TL bytes. Bound the
/// callback script/storage and construct its futures without blocking. Raw SDK
/// fixture errors are normalized before they can become a public ClientError.
pub fn client<F, Fut>(session: Session, options: ClientOptions, callback: F) -> Result<Client>
where
    F: Fn(i32, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Vec<u8>, InvocationError>> + Send + 'static,
{
    Client::with_factory(
        session,
        options,
        Box::new(move |session, api_id| SenderPool::with_test_rpc(session, api_id, callback)),
    )
}
