//! Synthetic encrypted session, immutable host bytes, real TL upload and cache.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tg_agent::{
    journal::{Journal, JournalOptions},
    mtproto::{
        Action, Budget, Call, Executor, ExecutorOptions, Grant, PeerId, PeerKind, PeerScope,
        PolicyStore, Principal, Request, SendDocumentArgs,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};

fn document_receipt() -> Vec<u8> {
    let mut v = tl::types::MessageMediaDocument::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&tl::types::Document::CONSTRUCTOR_ID.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&1i64.to_le_bytes());
    v.extend_from_slice(&777i64.to_le_bytes());
    v.extend_from_slice(&b"synthetic reference".to_vec().to_bytes());
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&"application/octet-stream".to_owned().to_bytes());
    v.extend_from_slice(&3i64.to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(
        &vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "sample.bin".into(),
            },
        )]
        .to_bytes(),
    );
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id: 1,
        pts: 1,
        pts_count: 1,
        date: 123,
        media: Some(tl::enums::MessageMedia::from_bytes(&v).expect("synthetic media")),
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("agent.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let identity = ProfileIdentity::mtproto_user(42)?;
    let feed = Feed::open(journal.clone(), "personal", identity, Options::default()).await?;
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("synthetic", 1)?,
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatched = calls.clone();
    let client = test_kit::client(session, ClientOptions::default(), move |_, bytes| {
        let calls = dispatched.clone();
        async move {
            assert!(
                calls.fetch_add(1, Ordering::SeqCst) < 4,
                "bounded fixture has no fallback"
            );
            let constructor = u32::from_le_bytes(bytes[..4].try_into().expect("TL constructor"));
            if constructor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
                let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
                v.extend_from_slice(&(1u32 << 10).to_le_bytes());
                v.extend_from_slice(&0u32.to_le_bytes());
                v.extend_from_slice(&42i64.to_le_bytes());
                return Ok(
                    vec![tl::enums::User::from_bytes(&v).expect("synthetic self")].to_bytes(),
                );
            }
            if constructor == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID {
                let part =
                    tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).expect("part");
                assert_eq!(part.bytes, b"abc");
                assert_eq!(part.file_part, 0);
                return Ok(true.to_bytes());
            }
            assert_eq!(
                constructor,
                tl::functions::messages::SendMedia::CONSTRUCTOR_ID
            );
            Ok(document_receipt())
        }
    })?;
    let principal = Principal::new("offline_operator")?;
    let policy = PolicyStore::default();
    let executor = Executor::new(policy.clone(), ExecutorOptions::default())?;
    executor.register_profile(feed, client.clone())?;
    let result: Result<_, Box<dyn std::error::Error>> = async {
        let expiry =
            i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())? + 60_000;
        let file = executor
            .register_file(
                &principal,
                "personal",
                "sample.bin",
                b"abc".to_vec(),
                expiry,
            )
            .await?;
        let peer = PeerId::new(PeerKind::User, 42)?;
        policy.replace_mtproto(
            &principal,
            "personal",
            Grant::new(identity, Budget::new("daily")?)?
                .allow(Action::SendDocument)
                .peers(PeerScope::only([peer]))
                .files([file.handle()]),
        )?;
        let request = Request::new(
            "personal",
            Some("stable-document".into()),
            Call::SendDocument(SendDocumentArgs {
                peer,
                file_handle: file.handle().into(),
                caption: Some("Synthetic document".into()),
            }),
        )?;
        let first = executor.execute(&principal, &request, None).await?;
        assert!(first.operation().expect("receipt").message().is_some());
        executor.discard_file(&principal, "personal", file.handle())?;
        let cached = executor.execute(&principal, &request, None).await?;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "two self checks, one part, one media; cache has no RPC"
        );
        Ok(cached)
    }
    .await;
    executor.close().await?;
    client.close().await?;
    journal.close().await?;
    println!("{}", serde_json::to_string(&result?)?);
    Ok(())
}
