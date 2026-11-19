use super::*;
use crate::{AccountId, AccountIdentity, AccountKind, AppCredentials, BotToken};
use std::sync::atomic::AtomicUsize;
#[path = "interactive_tests.rs"]
mod interactive;
#[path = "logout_tests.rs"]
mod logout;

fn expected(bot: bool) -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: if bot {
            AccountKind::Bot
        } else {
            AccountKind::User
        },
    }
}
fn credentials() -> AppCredentials {
    AppCredentials::new(1, "0123456789abcdef0123456789abcdef").unwrap()
}
fn user(id: i64, flags: u32) -> tl::enums::User {
    let mut bytes = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    if flags & 1 != 0 {
        bytes.extend_from_slice(&123i64.to_le_bytes());
    }
    if flags & (1 << 14) != 0 {
        bytes.extend_from_slice(&0i32.to_le_bytes());
    }
    tl::enums::User::from_bytes(&bytes).unwrap()
}
fn authorization(user: tl::enums::User) -> Vec<u8> {
    let mut bytes = tl::types::auth::Authorization::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&user.to_bytes());
    bytes
}

#[test]
fn credentials_are_validated_and_redacted() {
    let app = credentials();
    let token = BotToken::new("42:synthetic_private_token").unwrap();
    assert!(!format!("{app:?} {token:?}").contains("0123456789abcdef"));
    assert!(!format!("{app:?} {token:?}").contains("synthetic_private_token"));
    for hash in ["", "private", "0123456789abcdef0123456789abcdeg"] {
        assert_eq!(
            AppCredentials::new(1, hash).unwrap_err().outcome(),
            Outcome::NotSent
        );
    }
    assert!(AppCredentials::new(0, "0123456789abcdef0123456789abcdef").is_err());
    for token in [
        "",
        "0:secret",
        "-1:secret",
        "42:",
        "42:private secret",
        "42:secret:extra",
    ] {
        assert!(BotToken::new(token).is_err());
    }
    assert!(BotToken::new(format!("42:{}", "x".repeat(513))).is_err());
}

#[tokio::test]
async fn bot_login_commits_expected_identity_and_self_peer_before_success() {
    let (directory, session) = session().await;
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        |_, bytes| async move {
            let request =
                tl::functions::auth::ImportBotAuthorization::from_boxed_bytes(&bytes).unwrap();
            assert_eq!(request.api_id, 1);
            assert_eq!(request.api_hash, "0123456789abcdef0123456789abcdef");
            assert_eq!(request.bot_auth_token, "42:synthetic_private_token");
            Ok(authorization(user(42, 1 | (1 << 10) | (1 << 14))))
        },
    );
    let identity = client
        .sign_in_bot(
            credentials(),
            BotToken::new("42:synthetic_private_token").unwrap(),
            expected(true),
        )
        .await
        .unwrap();
    assert_eq!(identity, expected(true));
    assert_eq!(session.remembered_account().unwrap(), Some(identity));
    client.close().await.unwrap();
    let restored = Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(restored.remembered_account().unwrap(), Some(identity));
    restored.close().await.unwrap();
}

#[tokio::test]
async fn identity_is_live_and_local_mismatches_never_dispatch() {
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        observed.fetch_add(1, Ordering::SeqCst);
        async move {
            let request = tl::functions::users::GetUsers::from_boxed_bytes(&bytes).unwrap();
            assert_eq!(request.id.len(), 1);
            assert!(matches!(request.id[0], tl::enums::InputUser::UserSelf));
            Ok(vec![user(42, 1 | (1 << 10))].to_bytes())
        }
    });
    assert_eq!(
        client.verify_account(expected(false)).await.unwrap(),
        expected(false)
    );
    assert_eq!(
        client.verify_account(expected(false)).await.unwrap(),
        expected(false)
    );
    let failure = client.verify_account(expected(true)).await.unwrap_err();
    assert_eq!(failure.outcome(), Outcome::NotSent);
    let failure = client
        .sign_in_bot(
            credentials(),
            BotToken::new("43:secret").unwrap(),
            expected(true),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.outcome(), Outcome::NotSent);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    client.close().await.unwrap();
}

#[tokio::test]
async fn foreign_partial_and_missing_self_identity_fail_closed() {
    for flags in [
        1,
        1 | (1 << 10) | (1 << 13),
        1 | (1 << 10) | (1 << 20),
        1 | (1 << 10) | (1 << 14),
    ] {
        let (_directory, session) = session().await;
        let client = fixture(session, ClientOptions::default(), move |_, _| async move {
            Ok(vec![user(42, flags)].to_bytes())
        });
        let failure = client.verify_account(expected(false)).await.unwrap_err();
        assert_eq!(failure.outcome(), Outcome::Unknown);
        assert!(
            client
                .invoke(&tl::functions::Ping { ping_id: 1 })
                .await
                .is_err()
        );
        client.close().await.unwrap();
    }
    for count in [0, 2] {
        let (_directory, session) = session().await;
        let client = fixture(session, ClientOptions::default(), move |_, _| async move {
            Ok(vec![user(42, 1 | (1 << 10)); count].to_bytes())
        });
        assert_eq!(
            client
                .verify_account(expected(false))
                .await
                .unwrap_err()
                .outcome(),
            Outcome::Unknown
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn a_cancelled_login_keeps_its_gate_through_identity_commit() {
    let (_directory, session) = session().await;
    let observed_session = session.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = entered.clone();
    let gate = release.clone();
    let client = fixture(
        session,
        ClientOptions::default().max_pending(2),
        move |_, bytes| {
            let seen = seen.clone();
            let gate = gate.clone();
            let session = observed_session.clone();
            async move {
                if bytes[..4]
                    == tl::functions::auth::ImportBotAuthorization::CONSTRUCTOR_ID.to_le_bytes()
                {
                    seen.notify_one();
                    gate.notified().await;
                    Ok(authorization(user(42, 1 | (1 << 10) | (1 << 14))))
                } else {
                    assert_eq!(session.remembered_account().unwrap(), Some(expected(true)));
                    Ok(pong(&bytes))
                }
            }
        },
    );
    let login = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .sign_in_bot(
                    credentials(),
                    BotToken::new("42:secret").unwrap(),
                    expected(true),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    login.abort();
    let _ = login.await;
    let mut raw = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 1 }).await }
    });
    let early = tokio::time::timeout(Duration::from_millis(30), &mut raw).await;
    release.notify_one();
    let (completed_early, result) = match early {
        Ok(result) => (true, result),
        Err(_) => (
            false,
            tokio::time::timeout(Duration::from_secs(1), raw)
                .await
                .unwrap(),
        ),
    };
    client.close().await.unwrap();
    assert!(
        !completed_early,
        "raw work must wait for the cancelled owned login"
    );
    result.unwrap().unwrap();
}

#[tokio::test]
async fn unexpected_signup_and_foreign_bot_authorization_never_become_success() {
    for foreign in [false, true] {
        let (_directory, session) = session().await;
        let client = fixture(session, ClientOptions::default(), move |_, _| async move {
            if foreign {
                Ok(authorization(user(43, 1 | (1 << 10) | (1 << 14))))
            } else {
                let mut bytes = tl::types::auth::AuthorizationSignUpRequired::CONSTRUCTOR_ID
                    .to_le_bytes()
                    .to_vec();
                bytes.extend_from_slice(&0u32.to_le_bytes());
                Ok(bytes)
            }
        });
        let failure = client
            .sign_in_bot(
                credentials(),
                BotToken::new("42:secret").unwrap(),
                expected(true),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.outcome(), Outcome::Unknown);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn app_binding_and_encoded_limits_refuse_login_without_dispatch() {
    for limit in [4, 1024] {
        let (_directory, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(
            session,
            ClientOptions::default().max_request_bytes(limit),
            move |_, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                async { Ok(Vec::new()) }
            },
        );
        let credentials = AppCredentials::new(
            if limit == 4 { 1 } else { 2 },
            "0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        let failure = client
            .sign_in_bot(
                credentials,
                BotToken::new("42:secret").unwrap(),
                expected(true),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.outcome(), Outcome::NotSent);
        assert_eq!(failure.kind(), ClientErrorKind::InvalidInput);
        client.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn uncertain_login_is_not_replayed_and_remote_descriptions_are_discarded() {
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        async {
            Err(grammers_mtsender::InvocationError::Rpc(
                grammers_mtsender::RpcError {
                    code: 500,
                    name: "PRIVATE_SYNTHETIC_SECRET".into(),
                    value: None,
                    caused_by: None,
                },
            ))
        }
    });
    let failure = client
        .sign_in_bot(
            credentials(),
            BotToken::new("42:secret").unwrap(),
            expected(true),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.outcome(), Outcome::Unknown);
    assert_eq!(failure.rpc_code(), Some(500));
    assert!(!format!("{failure} {failure:?}").contains("PRIVATE_SYNTHETIC_SECRET"));
    assert!(std::error::Error::source(&failure).is_none());
    assert_eq!(
        client
            .invoke(&tl::functions::Ping { ping_id: 1 })
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    client.close().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inconsistent_login_fences_before_waking_waiting_raw_jobs() {
    let (_directory, session) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = entered.clone();
    let gate = release.clone();
    let dispatched = calls.clone();
    let options = ClientOptions {
        before_fence: Some(Duration::from_millis(100)),
        ..ClientOptions::default()
    };
    let client = fixture(session, options, move |_, bytes| {
        let seen = seen.clone();
        let gate = gate.clone();
        dispatched.fetch_add(1, Ordering::SeqCst);
        async move {
            if bytes[..4]
                == tl::functions::auth::ImportBotAuthorization::CONSTRUCTOR_ID.to_le_bytes()
            {
                seen.notify_one();
                gate.notified().await;
                Ok(authorization(user(43, 1 | (1 << 10) | (1 << 14))))
            } else {
                Ok(pong(&bytes))
            }
        }
    });
    let login = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .sign_in_bot(
                    credentials(),
                    BotToken::new("42:secret").unwrap(),
                    expected(true),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    let raw = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 1 }).await }
    });
    // Keep the gate held long enough for the reader to enter its owned job.
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.notify_one();
    let login = tokio::time::timeout(Duration::from_secs(1), login)
        .await
        .unwrap()
        .unwrap();
    let raw = tokio::time::timeout(Duration::from_secs(1), raw)
        .await
        .unwrap()
        .unwrap();
    client.close().await.unwrap();
    assert_eq!(login.unwrap_err().outcome(), Outcome::Unknown);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no raw dispatch after inconsistent authorization"
    );
    assert_eq!(raw.unwrap_err().outcome(), Outcome::NotSent);
}

#[tokio::test]
async fn bot_login_follows_one_confirmed_known_migration_and_persists_the_home_dc() {
    let (directory, session) = session().await;
    let datacenters = Arc::new(Mutex::new(Vec::new()));
    let seen = datacenters.clone();
    let client = fixture(session, ClientOptions::default(), move |dc, _| {
        let first = {
            let mut calls = seen.lock().unwrap();
            calls.push(dc);
            calls.len() == 1
        };
        async move {
            if first {
                Err(grammers_mtsender::InvocationError::Rpc(
                    grammers_mtsender::RpcError {
                        code: 303,
                        name: "USER_MIGRATE".into(),
                        value: Some(3),
                        caused_by: None,
                    },
                ))
            } else {
                Ok(authorization(user(42, 1 | (1 << 10) | (1 << 14))))
            }
        }
    });
    client
        .sign_in_bot(
            credentials(),
            BotToken::new("42:secret").unwrap(),
            expected(true),
        )
        .await
        .unwrap();
    assert_eq!(*datacenters.lock().unwrap(), vec![2, 3]);
    client.close().await.unwrap();
    let restored = Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    let client = fixture(restored, ClientOptions::default(), |dc, bytes| async move {
        assert_eq!(dc, 3);
        Ok(pong(&bytes))
    });
    client
        .invoke(&tl::functions::Ping { ping_id: 1 })
        .await
        .unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn unrecognized_same_missing_and_repeated_migrations_never_loop() {
    for (code, name, value, attempts) in [
        (303, "PHONE_MIGRATE", Some(3), 2),
        (303, "PHONE_MIGRATE", Some(2), 1),
        (303, "PHONE_MIGRATE", Some(0), 1),
        (303, "PHONE_MIGRATE", Some(99999), 1),
        (303, "PHONE_MIGRATE", None, 1),
        (303, "FILE_MIGRATE", Some(3), 1),
        (500, "PHONE_MIGRATE", Some(3), 1),
    ] {
        let (_directory, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(session, ClientOptions::default(), move |_, _| {
            let attempt = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                Err(grammers_mtsender::InvocationError::Rpc(
                    grammers_mtsender::RpcError {
                        code,
                        name: name.into(),
                        value: if attempt == 0 { value } else { Some(4) },
                        caused_by: None,
                    },
                ))
            }
        });
        let failure = client
            .sign_in_bot(
                credentials(),
                BotToken::new("42:secret").unwrap(),
                expected(true),
            )
            .await
            .unwrap_err();
        assert_eq!(
            failure.outcome(),
            if code == 500 {
                Outcome::Unknown
            } else {
                Outcome::Rejected
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), attempts);
        client.close().await.unwrap();
    }
}
