#![cfg(feature = "test-kit")]
use std::time::Duration;
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, ClientOptions, Session, SessionBinding, SessionKey,
    SessionOptions, UpdateItem, UpdateMode, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
async fn open(path: &std::path::Path) -> Session {
    Session::open(
        path,
        SessionBinding::new("synthetic", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap()
}
fn expected() -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: AccountKind::User,
    }
}
fn reply(bytes: &[u8]) -> Vec<u8> {
    if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
        let mut value = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
        value.extend_from_slice(&(1u32 << 10).to_le_bytes());
        value.extend_from_slice(&0u32.to_le_bytes());
        value.extend_from_slice(&42i64.to_le_bytes());
        vec![tl::enums::User::from_bytes(&value).unwrap()].to_bytes()
    } else {
        assert_eq!(
            &bytes[..4],
            &tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes()
        );
        tl::enums::updates::State::State(tl::types::updates::State {
            pts: 1,
            qts: 0,
            seq: 0,
            date: 100,
            unread_count: 0,
        })
        .to_bytes()
    }
}
#[tokio::test]
async fn source_fingerprint_survives_reopen_and_distinguishes_other_stores() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("one.sqlite");
    let session = open(&path).await;
    let source = session.storage_fingerprint();
    assert_ne!(source, [0; 32]);
    session.close().await.unwrap();
    assert_eq!(session.storage_fingerprint(), source);
    let reopened = open(&path).await;
    assert_eq!(reopened.storage_fingerprint(), source);
    let other = open(&directory.path().join("two.sqlite")).await;
    assert_ne!(other.storage_fingerprint(), source);
    reopened.close().await.unwrap();
    other.close().await.unwrap();
}
#[tokio::test]
async fn owned_fixture_exposes_only_stable_source_account_and_ordinal_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    let session = open(&path).await;
    let source = session.storage_fingerprint();
    let client = test_kit::client(session, ClientOptions::default(), |_, bytes| async move {
        Ok(reply(&bytes))
    })
    .unwrap();
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::StartNow)
        .unwrap();
    assert_eq!(receiver.identity(), expected());
    assert_eq!(receiver.source_id(), source);
    assert_eq!(receiver.request_budget(), Duration::from_secs(30));
    let Some(UpdateItem::Batch(first)) = receiver.next_batch(Duration::ZERO).await.unwrap() else {
        panic!("baseline")
    };
    assert_eq!(first.identity(), expected());
    assert_eq!(first.source_id(), source);
    assert!(first.ordinal() > 0);
    let id = first.id();
    let ordinal = first.ordinal();
    client.close().await.unwrap();
    drop(receiver);
    let reopened = open(&path).await;
    let client = test_kit::client(reopened, ClientOptions::default(), |_, bytes| async move {
        Ok(reply(&bytes))
    })
    .unwrap();
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let Some(UpdateItem::Batch(replay)) = receiver.next_batch(Duration::ZERO).await.unwrap() else {
        panic!("replay")
    };
    assert_eq!(
        (replay.id(), replay.ordinal(), replay.source_id()),
        (id, ordinal, source)
    );
    assert!(
        receiver.acknowledge(&first).await.is_err(),
        "metadata does not authorize a foreign generation's ACK"
    );
    receiver.acknowledge(&replay).await.unwrap();
    client.close().await.unwrap();
}
