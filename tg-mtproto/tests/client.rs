use tg_mtproto::{
    Client, ClientOptions, Session, SessionBinding, SessionErrorKind, SessionKey, SessionOptions,
};

#[tokio::test]
async fn client_claim_is_unique_and_an_unstarted_drop_releases_it_without_network() {
    let directory = tempfile::tempdir().unwrap();
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("demo", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    let first = Client::new(session.clone(), ClientOptions::default()).unwrap();
    assert!(Client::new(session.clone(), ClientOptions::default()).is_err());
    assert_eq!(
        session.close().await.unwrap_err().kind(),
        SessionErrorKind::Owned
    );
    drop(first);
    let second = Client::new(session.clone(), ClientOptions::default()).unwrap();
    second.close().await.unwrap();
    assert_eq!(
        session.remembered_account().unwrap_err().kind(),
        SessionErrorKind::Closed
    );
}
