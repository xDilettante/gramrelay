//! Synthetic encrypted session + real TL codecs + authorized durable execution.
//! No sockets, credentials or existing private session files are used.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tg_agent::{
    executor::ExecutorError,
    journal::{Journal, JournalOptions, OperationState},
    mtproto::{
        Action, Budget, Call, Executor, ExecutorOptions, Grant, PeerId, PeerKind, PeerScope,
        PolicyStore, Principal, Request, SendMessageArgs,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed = Feed::open(
        journal.clone(),
        "personal",
        ProfileIdentity::mtproto_user(42)?,
        Options::default(),
    )
    .await?;
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("synthetic", 1)?,
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let script = calls.clone();
    let client = test_kit::client(session, ClientOptions::default(), move |_, bytes| {
        let script = script.clone();
        async move {
            assert!(
                script.fetch_add(1, Ordering::SeqCst) < 3,
                "bounded fixture has no fallback"
            );
            if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                let mut user = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
                user.extend_from_slice(&(1u32 << 10).to_le_bytes());
                user.extend_from_slice(&0u32.to_le_bytes());
                user.extend_from_slice(&42i64.to_le_bytes());
                Ok(vec![tl::enums::User::from_bytes(&user).expect("synthetic self")].to_bytes())
            } else {
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMessage::CONSTRUCTOR_ID.to_le_bytes()
                );
                Ok(
                    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
                        out: true,
                        id: 1,
                        pts: 1,
                        pts_count: 1,
                        date: 123,
                        media: None,
                        entities: None,
                        ttl_period: None,
                    })
                    .to_bytes(),
                )
            }
        }
    })?;
    let principal = Principal::new("offline_operator")?;
    let peer = PeerId::new(PeerKind::User, 42)?;
    let policy = PolicyStore::default();
    policy.replace_mtproto(
        &principal,
        "personal",
        Grant::new(ProfileIdentity::mtproto_user(42)?, Budget::new("daily")?)?
            .allow(Action::SendMessage)
            .peers(PeerScope::only([peer])),
    )?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(feed, client.clone())?;
    let request = Request::new(
        "personal",
        Some("stable-offline-key".into()),
        Call::SendMessage(SendMessageArgs {
            peer,
            text: "Synthetic example".into(),
            disable_notification: false,
        }),
    )?;
    let result: Result<_, ExecutorError> = async {
        let first = executor.execute(&principal, &request, None).await?;
        let second = executor.execute(&principal, &request, None).await?;
        assert_eq!(
            first.operation().unwrap().state(),
            OperationState::Succeeded
        );
        assert_eq!(
            second.operation().unwrap().state(),
            OperationState::Succeeded
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "second invocation reads the durable receipt"
        );
        Ok(second)
    }
    .await;
    executor.close().await?;
    client.close().await?;
    journal.close().await?;
    println!("{}", serde_json::to_string(&result?)?);
    Ok(())
}
