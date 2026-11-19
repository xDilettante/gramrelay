//! Real TL intake, opaque agent reference and one durable answer, without sockets.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    executor::ReadUpdatesArgs,
    journal::{Journal, JournalOptions},
    mtproto::{
        Action, AnswerCallbackQueryArgs, Budget, Call, Executor, ExecutorOptions, Grant, PeerId,
        PeerKind, PeerScope, PolicyStore, Principal, Request,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, ClientOptions, Session, SessionBinding, SessionKey,
    SessionOptions, UpdateMode, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};

fn state(pts: i32) -> tl::enums::updates::State {
    tl::types::updates::State {
        pts,
        qts: 0,
        seq: 0,
        date: 100 + pts,
        unread_count: 0,
    }
    .into()
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let identity = ProfileIdentity::mtproto_bot(42)?;
    let feed = Feed::open(journal.clone(), "support", identity, Options::default()).await?;
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("synthetic", 1)?,
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await?;
    let answers = Arc::new(AtomicUsize::new(0));
    let calls = answers.clone();
    let client = test_kit::client(session, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            let ctor = u32::from_le_bytes(bytes[..4].try_into().expect("TL constructor"));
            if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
                let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
                v.extend_from_slice(&((1u32 << 10) | (1 << 14)).to_le_bytes());
                v.extend_from_slice(&0u32.to_le_bytes());
                v.extend_from_slice(&42i64.to_le_bytes());
                v.extend_from_slice(&0i32.to_le_bytes());
                return Ok(
                    vec![tl::enums::User::from_bytes(&v).expect("synthetic bot")].to_bytes(),
                );
            }
            if ctor == tl::functions::updates::GetState::CONSTRUCTOR_ID {
                return Ok(state(1).to_bytes());
            }
            if ctor == tl::functions::updates::GetDifference::CONSTRUCTOR_ID {
                return Ok(tl::enums::updates::Difference::Difference(
                    tl::types::updates::Difference {
                        new_messages: vec![],
                        new_encrypted_messages: vec![],
                        chats: vec![],
                        users: vec![],
                        state: state(2),
                        other_updates: vec![
                            tl::types::UpdateBotCallbackQuery {
                                query_id: 0,
                                user_id: 99,
                                peer: tl::types::PeerUser { user_id: 7 }.into(),
                                msg_id: 3,
                                chat_instance: 888,
                                data: Some(b"confirm".to_vec()),
                                game_short_name: None,
                            }
                            .into(),
                        ],
                    },
                )
                .to_bytes());
            }
            assert_eq!(
                ctor,
                tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID
            );
            let request = tl::functions::messages::SetBotCallbackAnswer::from_bytes(&bytes[4..])
                .expect("callback intent");
            assert_eq!(request.query_id, 0);
            assert_eq!(request.message.as_deref(), Some("Confirmed"));
            assert_eq!(
                calls.fetch_add(1, Ordering::SeqCst),
                0,
                "fixture permits one dispatch"
            );
            Ok(true.to_bytes())
        }
    })?;
    let principal = Principal::new("offline_operator")?;
    let policy = PolicyStore::default();
    policy.replace_mtproto(
        &principal,
        "support",
        Grant::new(identity, Budget::new("daily")?)?
            .allow(Action::ReadUpdates)
            .allow(Action::AnswerCallbackQuery)
            .peers(PeerScope::only([PeerId::new(PeerKind::User, 7)?])),
    )?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(feed.clone(), client.clone())?;
    let result: Result<_, Box<dyn std::error::Error>> = async {
        let mut source = feed.claim_source().await?;
        let mut receiver = client
            .account(AccountIdentity {
                id: AccountId::new(42)?,
                kind: AccountKind::Bot,
            })
            .updates(UpdateMode::StartNow)?;
        for _ in 0..2 {
            feed.ingest(&mut source, &mut receiver, Duration::ZERO)
                .await?;
        }
        feed.release_source(&source).await?;
        let read = Request::new(
            "support",
            None,
            Call::ReadUpdates(ReadUpdatesArgs {
                cursor: None,
                limit: 100,
            }),
        )?;
        let page = executor.execute(&principal, &read, None).await?;
        let reference = page.read_page().expect("updates").events()[0]
            .callback_reference()
            .expect("observed authorized callback");
        let answer = Request::new(
            "support",
            Some("stable-answer".into()),
            Call::AnswerCallbackQuery(AnswerCallbackQueryArgs {
                reference: reference.into(),
                text: Some("Confirmed".into()),
                show_alert: false,
            }),
        )?;
        let first = executor.execute(&principal, &answer, None).await?;
        assert_eq!(
            first.operation().expect("receipt").acknowledged(),
            Some(true)
        );
        let cached = executor.execute(&principal, &answer, None).await?;
        assert_eq!(answers.load(Ordering::SeqCst), 1);
        Ok(cached)
    }
    .await;
    executor.close().await?;
    client.close().await?;
    journal.close().await?;
    println!("{}", serde_json::to_string(&result?)?);
    Ok(())
}
