use super::*;

async fn verify_dc(
    client: &Client,
    expected: AccountIdentity,
    dc: i32,
) -> std::result::Result<(), ClientError> {
    let session = client.inner.session.clone();
    let authorization = client.inner.authorization.clone();
    let gate = client.inner.dc_authorization.clone();
    let limit = client.inner.options.max_request_bytes;
    client
        .submit(client.reserve()?, move |context| async move {
            context.hold_authorization_read(authorization).await;
            super::super::super::auth::verify_in_context(&context, &session, expected, limit)
                .await?;
            context.verify_file_dc(expected, dc, gate, limit).await
        })
        .await
}
fn unauthorized() -> grammers_mtsender::InvocationError {
    grammers_mtsender::InvocationError::Rpc(grammers_mtsender::RpcError {
        code: 401,
        name: "AUTH_KEY_UNREGISTERED".into(),
        value: None,
        caused_by: None,
    })
}
fn imported(bot: bool) -> Vec<u8> {
    let mut bytes = tl::types::auth::Authorization::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&user(42, (1 << 10) | if bot { 1 << 14 } else { 0 }, 0).to_bytes());
    bytes
}

#[tokio::test]
async fn dc_copy_verifies_user_and_bot_without_changing_home_or_epoch() {
    for bot in [false, true] {
        let (_directory, stored) = session().await;
        let verified = Arc::new(AtomicBool::new(false));
        let state = verified.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(stored, ClientOptions::default(), move |dc, bytes| {
            let state = state.clone();
            let seen = seen.clone();
            async move {
                if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                    if dc == 3 && !state.load(Ordering::Acquire) {
                        return Err(unauthorized());
                    }
                    assert!(dc == 2 || dc == 3);
                    return Ok(
                        vec![user(42, (1 << 10) | if bot { 1 << 14 } else { 0 }, 0)].to_bytes()
                    );
                }
                if bytes[..4]
                    == tl::functions::auth::ExportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
                {
                    assert_eq!(dc, 2);
                    let request =
                        tl::functions::auth::ExportAuthorization::from_bytes(&bytes[4..]).unwrap();
                    assert_eq!(request.dc_id, 3);
                    return Ok(tl::enums::auth::ExportedAuthorization::Authorization(
                        tl::types::auth::ExportedAuthorization {
                            id: 42,
                            bytes: b"private exported ticket".to_vec(),
                        },
                    )
                    .to_bytes());
                }
                assert_eq!(dc, 3);
                assert_eq!(
                    bytes[..4],
                    tl::functions::auth::ImportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request =
                    tl::functions::auth::ImportAuthorization::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.id, 42);
                assert_eq!(request.bytes, b"private exported ticket");
                seen.fetch_add(1, Ordering::SeqCst);
                state.store(true, Ordering::Release);
                Ok(imported(bot))
            }
        });
        let expected = AccountIdentity {
            kind: if bot {
                AccountKind::Bot
            } else {
                AccountKind::User
            },
            ..expected()
        };
        verify_dc(&client, expected, 3).await.unwrap();
        verify_dc(&client, expected, 3).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.inner.auth_epoch.load(Ordering::Acquire), 0);
        // Every ordinary home self check above must still target DC 2.
        client.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_dc_confirmation_keeps_transition_guard_until_timeout_is_fenced() {
    let (_directory, stored) = session().await;
    let client = fixture(
        stored,
        ClientOptions {
            before_dc_confirm: Some(Duration::from_millis(250)),
            ..ClientOptions::default().request_timeout(Duration::from_millis(150))
        },
        |dc, bytes| async move {
            if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                return if dc == 2 {
                    Ok(vec![user(42, 1 << 10, 0)].to_bytes())
                } else {
                    Err(unauthorized())
                };
            }
            if bytes[..4] == tl::functions::auth::ExportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
            {
                return Ok(tl::enums::auth::ExportedAuthorization::Authorization(
                    tl::types::auth::ExportedAuthorization {
                        id: 42,
                        bytes: b"private ticket".to_vec(),
                    },
                )
                .to_bytes());
            }
            Ok(imported(false))
        },
    );
    let session = client.inner.session.clone();
    let authorization = client.inner.authorization.clone();
    let gate = client.inner.dc_authorization.clone();
    let witness = gate.clone();
    let confirmed = Arc::new(AtomicBool::new(false));
    let accepted = confirmed.clone();
    let retained = Arc::new(AtomicBool::new(false));
    let held = retained.clone();
    let result = client
        .submit(client.reserve().unwrap(), move |context| async move {
            context.hold_authorization_read(authorization).await;
            super::super::super::auth::verify_in_context(
                &context,
                &session,
                expected(),
                1024 * 1024,
            )
            .await?;
            let result = context
                .verify_file_dc(expected(), 3, gate, 1024 * 1024)
                .await;
            accepted.store(result.is_ok(), Ordering::SeqCst);
            held.store(witness.try_lock().is_err(), Ordering::SeqCst);
            result
        })
        .await;
    assert_eq!(result.unwrap_err().kind(), ClientErrorKind::Timeout);
    client.close().await.unwrap();
    assert!(
        !confirmed.load(Ordering::SeqCst),
        "late import must not clear pending authorization"
    );
    assert!(
        retained.load(Ordering::SeqCst),
        "late import must retain the DC guard until fencing"
    );
}

#[tokio::test]
async fn concurrent_dc_verification_copies_authorization_once() {
    let (_directory, stored) = session().await;
    let authorized = Arc::new(AtomicBool::new(false));
    let state = authorized.clone();
    let imports = Arc::new(AtomicUsize::new(0));
    let calls = imports.clone();
    let client = fixture(stored, ClientOptions::default(), move |dc, bytes| {
        let state = state.clone();
        let calls = calls.clone();
        async move {
            if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                if dc == 3 && !state.load(Ordering::Acquire) {
                    return Err(unauthorized());
                }
                return Ok(vec![user(42, 1 << 10, 0)].to_bytes());
            }
            if bytes[..4] == tl::functions::auth::ExportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
            {
                return Ok(tl::enums::auth::ExportedAuthorization::Authorization(
                    tl::types::auth::ExportedAuthorization {
                        id: 42,
                        bytes: vec![7; 32],
                    },
                )
                .to_bytes());
            }
            assert_eq!(
                bytes[..4],
                tl::functions::auth::ImportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
            );
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            state.store(true, Ordering::Release);
            Ok(imported(false))
        }
    });
    let (first, second) = tokio::join!(
        verify_dc(&client, expected(), 3),
        verify_dc(&client, expected(), 3)
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(imports.load(Ordering::SeqCst), 1);
    client.close().await.unwrap();
}

#[tokio::test]
async fn dc_copy_requires_exact_unregistered_and_valid_export_identity_and_ticket() {
    for case in 0..8 {
        let (_directory, stored) = session().await;
        let exports = Arc::new(AtomicUsize::new(0));
        let calls = exports.clone();
        let client = fixture(stored, ClientOptions::default(), move |dc, bytes| {
            let calls = calls.clone();
            async move {
                if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                    if dc == 2 {
                        return Ok(vec![user(42, 1 << 10, 0)].to_bytes());
                    }
                    return match case {
                        0 => Err(grammers_mtsender::InvocationError::Rpc(
                            grammers_mtsender::RpcError {
                                code: 400,
                                name: "AUTH_KEY_UNREGISTERED".into(),
                                value: None,
                                caused_by: None,
                            },
                        )),
                        1 => Err(grammers_mtsender::InvocationError::Rpc(
                            grammers_mtsender::RpcError {
                                code: 401,
                                name: "AUTH_KEY_UNREGISTERED".into(),
                                value: Some(1),
                                caused_by: None,
                            },
                        )),
                        2 => Err(grammers_mtsender::InvocationError::Rpc(
                            grammers_mtsender::RpcError {
                                code: 401,
                                name: "private foreign error".into(),
                                value: None,
                                caused_by: None,
                            },
                        )),
                        3 => Ok(vec![user(43, 1 << 10, 0)].to_bytes()),
                        _ => Err(unauthorized()),
                    };
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::auth::ExportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
                );
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(tl::enums::auth::ExportedAuthorization::Authorization(
                    tl::types::auth::ExportedAuthorization {
                        id: if case == 4 { 43 } else { 42 },
                        bytes: match case {
                            5 => vec![],
                            6 => vec![1; 16 * 1024 + 1],
                            _ => vec![1; 32],
                        },
                    },
                )
                .to_bytes())
            }
        });
        // Last case uses an unknown DC, refused before a target RPC.
        let error = verify_dc(&client, expected(), if case == 7 { 99 } else { 3 })
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private"));
        assert_eq!(
            exports.load(Ordering::SeqCst),
            if (4..=6).contains(&case) { 1 } else { 0 }
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn unknown_import_fences_waiters_even_after_its_caller_is_dropped() {
    let (_directory, stored) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let blocked = release.clone();
    let imports = Arc::new(AtomicUsize::new(0));
    let calls = imports.clone();
    let client = fixture(stored, ClientOptions::default(), move |dc, bytes| {
        let observed = observed.clone();
        let blocked = blocked.clone();
        let calls = calls.clone();
        async move {
            if bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
                if dc == 3 {
                    return Err(unauthorized());
                }
                return Ok(vec![user(42, 1 << 10, 0)].to_bytes());
            }
            if bytes[..4] == tl::functions::auth::ExportAuthorization::CONSTRUCTOR_ID.to_le_bytes()
            {
                return Ok(tl::enums::auth::ExportedAuthorization::Authorization(
                    tl::types::auth::ExportedAuthorization {
                        id: 42,
                        bytes: vec![7; 32],
                    },
                )
                .to_bytes());
            }
            calls.fetch_add(1, Ordering::SeqCst);
            observed.notify_one();
            blocked.notified().await;
            Err(grammers_mtsender::InvocationError::Rpc(
                grammers_mtsender::RpcError {
                    code: 500,
                    name: "private import error".into(),
                    value: None,
                    caused_by: None,
                },
            ))
        }
    });
    let owned = client.clone();
    let first = tokio::spawn(async move { verify_dc(&owned, expected(), 3).await });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    first.abort();
    let _ = first.await;
    let owned = client.clone();
    let second = tokio::spawn(async move { verify_dc(&owned, expected(), 3).await });
    release.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::Closed);
    assert_eq!(imports.load(Ordering::SeqCst), 1);
    assert!(client.inner.shared.closed.load(Ordering::Acquire));
    client.close().await.unwrap();
}
