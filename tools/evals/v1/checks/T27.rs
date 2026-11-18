use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions};

#[tokio::test]
async fn opens_same_encrypted_store_with_explicit_consumed_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unique-472.sqlite");
    let binding = SessionBinding::new("case-27", 112).unwrap();
    let key = || SessionKey::from_bytes([54; 32]);
    let session = solution::solve(&path, binding.clone(), key(), key())
        .await
        .unwrap();
    assert_eq!(session.binding().profile(), binding.profile());
    assert_eq!(session.binding().api_id(), binding.api_id());
    assert!(session.remembered_account().unwrap().is_none());
    let fingerprint = session.storage_fingerprint();
    session.close().await.unwrap();
    assert!(
        Session::open_existing(
            &path,
            binding.clone(),
            SessionKey::from_bytes([60; 32]),
            SessionOptions::default()
        )
        .await
        .is_err()
    );
    let reopened = Session::open_existing(&path, binding, key(), SessionOptions::default())
        .await
        .unwrap();
    assert_eq!(reopened.storage_fingerprint(), fingerprint);
    reopened.close().await.unwrap();

    let other = dir.path().join("reopen-key-probe.sqlite");
    let binding = SessionBinding::new("reopen-key-probe", 113).unwrap();
    assert!(
        solution::solve(
            &other,
            binding.clone(),
            key(),
            SessionKey::from_bytes([60; 32])
        )
        .await
        .is_err()
    );
    let created = Session::open_existing(&other, binding, key(), SessionOptions::default())
        .await
        .unwrap();
    created.close().await.unwrap();
}
