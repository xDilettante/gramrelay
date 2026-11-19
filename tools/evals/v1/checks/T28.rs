use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions};

#[tokio::test]
async fn invocation_refuses_missing_or_empty_store_and_opens_the_initialized_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing-87.sqlite");
    let binding = SessionBinding::new("case-28", 117).unwrap();
    let key = || SessionKey::from_bytes([21; 32]);
    assert!(
        solution::solve(&path, binding.clone(), key())
            .await
            .is_err()
    );
    assert!(!path.exists());
    std::fs::File::create(&path).unwrap();
    assert!(
        solution::solve(&path, binding.clone(), key())
            .await
            .is_err()
    );
    assert_eq!(path.metadata().unwrap().len(), 0);
    let created = Session::open(&path, binding.clone(), key(), SessionOptions::default())
        .await
        .unwrap();
    let fingerprint = created.storage_fingerprint();
    created.close().await.unwrap();
    let opened = solution::solve(&path, binding, key()).await.unwrap();
    assert_eq!(opened.storage_fingerprint(), fingerprint);
    opened.close().await.unwrap();
}
