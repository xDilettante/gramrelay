//! Offline source/API spike. It does not run a network sender or accept secrets.
use grammers_client::{
    Client, SenderPool,
    sender::{ConnectionParams, InvocationError, NoRetries, RetryContext, RetryPolicy, RpcError},
    session::{
        Session, SessionData,
        storages::MemorySession,
        types::{PeerId, PeerInfo, UpdateState, UpdatesState},
    },
    tl,
};
use std::{num::NonZeroU32, ops::ControlFlow, sync::Arc, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NoRetries;
    for error in [
        InvocationError::Rpc(RpcError {
            code: 420,
            name: "FLOOD_WAIT".into(),
            value: Some(42),
            caused_by: None,
        }),
        InvocationError::Io(std::io::Error::other("synthetic I/O")),
    ] {
        let decision = policy.should_retry(&RetryContext {
            fail_count: NonZeroU32::MIN,
            slept_so_far: Duration::ZERO,
            error,
        });
        assert!(matches!(decision, ControlFlow::Break(())));
    }
    let mut data = SessionData {
        home_dc: 1,
        updates_state: UpdatesState {
            pts: 10,
            qts: 11,
            date: 12,
            seq: 13,
            channels: vec![],
        },
        ..Default::default()
    };
    for option in data.dc_options.values_mut() {
        option.ipv4 = "127.0.0.1:9".parse()?;
        option.ipv6 = "[::ffff:127.0.0.1]:9".parse()?;
    }
    data.dc_options.get_mut(&1).unwrap().auth_key = Some([7; 256]);
    data.peer_infos.insert(
        PeerId::user(42).unwrap(),
        PeerInfo::User {
            id: 42,
            auth: None,
            bot: Some(false),
            is_self: Some(true),
        },
    );
    let session = Arc::new(MemorySession::default());
    data.import_to(session.as_ref()).await?;
    assert_eq!(session.home_dc_id()?, 1);
    let restored = session.dc_option(1)?.unwrap();
    assert_eq!(restored.auth_key, Some([7; 256]));
    let json = serde_json::to_vec(&restored)?;
    let decoded: grammers_session::types::DcOption = serde_json::from_slice(&json)?;
    assert_eq!(decoded.auth_key, restored.auth_key);
    assert_eq!(session.updates_state().await?.pts, 10);
    assert!(matches!(
        session.peer(PeerId::user(42).unwrap()).await?,
        Some(PeerInfo::User {
            id: 42,
            bot: Some(false),
            ..
        })
    ));
    // Upstream MemorySession uses exact map lookup and does not resolve the
    // self-user sentinel. Production must implement this Session requirement.
    assert!(session.peer(PeerId::self_user()).await?.is_none());
    session
        .set_update_state(UpdateState::All(data.updates_state))
        .await?;
    let SenderPool {
        runner,
        handle,
        updates: _,
    } = SenderPool::with_configuration(
        session,
        1,
        ConnectionParams {
            retry_policy: Box::new(NoRetries),
            ..Default::default()
        },
    );
    let client = Client::new(handle);
    // Construct a real raw request future, then drop it unpolled. The pool
    // runner is intentionally never polled: there can be no external I/O.
    drop(client.invoke(&tl::functions::Ping { ping_id: 42 }));
    drop(client);
    drop(runner);
    println!(
        "grammers 0.12.0: typed raw API, synthetic session import/serde/checkpoint, NoRetries flood/I/O and unpolled lazy shutdown verified; no network runner started."
    );
    Ok(())
}
