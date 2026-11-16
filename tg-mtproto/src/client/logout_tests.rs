use super::*;

fn logged_out() -> Vec<u8> {
    let mut bytes = tl::types::auth::LoggedOut::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}
async fn reopen(directory: &tempfile::TempDir) -> Session {
    Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap()
}
fn is_get_self(bytes: &[u8]) -> bool {
    bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes()
}

#[tokio::test]
async fn logout_intent_precedes_rpc_and_success_waits_for_closed_clean_storage() {
    let (directory, session) = session().await;
    let observed = session.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = entered.clone();
    let gate = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatched = calls.clone();
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        move |_, bytes| {
            let session = observed.clone();
            let seen = seen.clone();
            let gate = gate.clone();
            dispatched.fetch_add(1, Ordering::SeqCst);
            async move {
                if is_get_self(&bytes) {
                    return Ok(vec![user(42, 1 | (1 << 10) | (1 << 14))].to_bytes());
                }
                assert_eq!(bytes, tl::functions::auth::LogOut {}.to_bytes());
                assert!(
                    session.is_quarantined().unwrap(),
                    "intent must be durable before LogOut"
                );
                seen.notify_one();
                gate.notified().await;
                Ok(logged_out())
            }
        },
    );
    let logout = tokio::spawn({
        let client = client.clone();
        async move { client.sign_out(expected(true)).await }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        client
            .invoke(&tl::functions::Ping { ping_id: 1 })
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), logout)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        session.is_quarantined().is_err(),
        "success includes the native close barrier"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let restored = reopen(&directory).await;
    assert!(!restored.is_quarantined().unwrap());
    assert!(restored.remembered_account().unwrap().is_none());
    restored.close().await.unwrap();
}

#[tokio::test]
async fn unknown_and_refused_logout_remain_quarantined_across_encrypted_restart() {
    for rejected in [false, true] {
        let (directory, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(session, ClientOptions::default(), move |_, bytes| {
            seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if is_get_self(&bytes) {
                    return Ok(vec![user(42, 1 | (1 << 10) | (1 << 14))].to_bytes());
                }
                if rejected {
                    Err(grammers_mtsender::InvocationError::Rpc(
                        grammers_mtsender::RpcError {
                            code: 401,
                            name: "PRIVATE_REMOTE_TEXT".into(),
                            value: None,
                            caused_by: None,
                        },
                    ))
                } else {
                    Err(grammers_mtsender::InvocationError::Dropped(
                        "synthetic lost connection",
                    ))
                }
            }
        });
        let failure = client.sign_out(expected(true)).await.unwrap_err();
        assert_eq!(
            failure.outcome(),
            if rejected {
                Outcome::Rejected
            } else {
                Outcome::Unknown
            }
        );
        client.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let restored = reopen(&directory).await;
        assert!(restored.is_quarantined().unwrap());
        assert_eq!(restored.remembered_account().unwrap(), Some(expected(true)));
        assert_eq!(
            Client::new(restored.clone(), ClientOptions::default())
                .unwrap_err()
                .kind(),
            ClientErrorKind::Quarantined
        );
        restored.forget_quarantined_authorization().await.unwrap();
        assert!(restored.remembered_account().unwrap().is_none());
        restored.close().await.unwrap();
    }
}

#[tokio::test]
async fn cancelled_logout_waiter_does_not_cancel_confirmed_cleanup() {
    let (directory, session) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = entered.clone();
    let gate = release.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        let gate = gate.clone();
        async move {
            if is_get_self(&bytes) {
                return Ok(vec![user(42, 1 | (1 << 10) | (1 << 14))].to_bytes());
            }
            seen.notify_one();
            gate.notified().await;
            Ok(logged_out())
        }
    });
    let logout = tokio::spawn({
        let client = client.clone();
        async move { client.sign_out(expected(true)).await }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    logout.abort();
    let _ = logout.await;
    release.notify_one();
    // Wait for the admitted RPC to finish before asking for the close barrier.
    tokio::time::timeout(Duration::from_secs(1), async {
        while !client.inner.shared.closed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close().await.unwrap();
    let restored = reopen(&directory).await;
    assert!(!restored.is_quarantined().unwrap());
    assert!(restored.remembered_account().unwrap().is_none());
    restored.close().await.unwrap();
}

#[tokio::test]
async fn failed_preliminary_identity_check_sends_no_logout_or_quarantine() {
    let (_directory, session) = session().await;
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        |_, bytes| async move {
            assert!(is_get_self(&bytes));
            Err(grammers_mtsender::InvocationError::Rpc(
                grammers_mtsender::RpcError {
                    code: 401,
                    name: "AUTH_KEY_UNREGISTERED".into(),
                    value: None,
                    caused_by: None,
                },
            ))
        },
    );
    assert_eq!(
        client.sign_out(expected(true)).await.unwrap_err().outcome(),
        Outcome::Rejected
    );
    assert!(!session.is_quarantined().unwrap());
    client.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_timeout_seals_late_final_acceptance_without_clearing_quarantine() {
    let (directory, session) = session().await;
    let options = ClientOptions {
        before_accept: Some(Duration::from_millis(200)),
        ..ClientOptions::default().request_timeout(Duration::from_millis(80))
    };
    let client = fixture(session, options, |_, bytes| async move {
        if is_get_self(&bytes) {
            Ok(vec![user(42, 1 | (1 << 10) | (1 << 14))].to_bytes())
        } else {
            Ok(logged_out())
        }
    });
    let failure = client.sign_out(expected(true)).await.unwrap_err();
    client.close().await.unwrap();
    assert_eq!(failure.kind(), ClientErrorKind::Timeout);
    assert_eq!(failure.outcome(), Outcome::Unknown);
    let restored = reopen(&directory).await;
    assert!(
        restored.is_quarantined().unwrap(),
        "a sealed timeout cannot later arm successful credential clearing"
    );
    restored.close().await.unwrap();
}

#[tokio::test]
async fn an_unanswered_logout_times_out_and_joins_its_child_before_recovery() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (directory, session) = session().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = dropped.clone();
    let client = fixture(
        session,
        ClientOptions::default().request_timeout(Duration::from_millis(300)),
        move |_, bytes| {
            let dropped = observed.clone();
            async move {
                if is_get_self(&bytes) {
                    return Ok(vec![user(42, 1 | (1 << 10) | (1 << 14))].to_bytes());
                }
                let _guard = Dropped(dropped);
                std::future::pending().await
            }
        },
    );
    let failure = client.sign_out(expected(true)).await.unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Timeout);
    assert_eq!(failure.outcome(), Outcome::Unknown);
    client.close().await.unwrap();
    assert!(dropped.load(Ordering::Acquire));
    let restored = reopen(&directory).await;
    assert!(restored.is_quarantined().unwrap());
    restored.close().await.unwrap();
}
