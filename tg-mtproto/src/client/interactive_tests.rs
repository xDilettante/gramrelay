use super::*;
use crate::auth::{
    AuthStep, LoginCode, Password, PasswordPromptReason, PhoneNumber, UserLoginTarget,
};

fn sent_code() -> Vec<u8> {
    tl::enums::auth::SentCode::Code(tl::types::auth::SentCode {
        r#type: tl::enums::auth::SentCodeType::App(tl::types::auth::SentCodeTypeApp { length: 5 }),
        phone_code_hash: "PRIVATE_CHALLENGE_HASH".into(),
        next_type: None,
        timeout: Some(30),
    })
    .to_bytes()
}
fn code_challenge(step: AuthStep) -> crate::auth::CodeChallenge {
    let AuthStep::CodeRequired(challenge) = step else {
        panic!("expected code challenge");
    };
    challenge
}
fn rpc(code: i32, name: &str) -> grammers_mtsender::InvocationError {
    grammers_mtsender::InvocationError::Rpc(grammers_mtsender::RpcError {
        code,
        name: name.into(),
        value: None,
        caused_by: None,
    })
}

// Public Telegram DH prime from the pinned grammers-mtproto 0.12.0 source.
fn prime() -> Vec<u8> {
    let hex = concat!(
        "C71CAEB9C6B1C9048E6C522F70F13F73980D40238E3E21C14934D037563D930F",
        "48198A0AA7C14058229493D22530F4DBFA336F6E0AC925139543AED44CCE7C37",
        "20FD51F69458705AC68CD4FE6B6B13ABDC9746512969328454F18FAF8C595F64",
        "2477FE96BB2A941D5BCD1D4AC8CC49880708FA9B378E3C4F3A9060BEE67CF9A4",
        "A4A695811051907E162753B56B0F6B410DBA74D8A84B2A14B3144E0EF1284754",
        "FD17ED950D5965B4B9DD46582DB1178D169C6BC465B0D6FF9CA3928FEF5B9AE4",
        "E418FC15E83EBEA0F87FA9FF5EED70050DED2849F47BF959D956850CE929851F",
        "0D8115F635B105EE2E4E15D04B2454BF6F4FADF034B10403119CD8E3B92FCC5B"
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
fn password_info(id: i64) -> Vec<u8> {
    tl::enums::account::Password::Password(tl::types::account::Password {
        has_recovery: false,
        has_secure_values: false,
        has_password: true,
        current_algo: Some(
            tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(
                tl::types::PasswordKdfAlgoSha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow {
                    salt1: b"synthetic_salt_1".to_vec(),
                    salt2: b"synthetic_salt_2".to_vec(),
                    g: 3,
                    p: prime(),
                },
            ),
        ),
        srp_b: Some(vec![0x5a; 256]),
        srp_id: Some(id),
        hint: Some("SYNTHETIC_PRIVATE_HINT".into()),
        email_unconfirmed_pattern: None,
        new_algo: tl::enums::PasswordKdfAlgo::Unknown,
        new_secure_algo: tl::enums::SecurePasswordKdfAlgo::Unknown,
        secure_random: vec![0xa5; 32],
        pending_reset_date: None,
        login_email_pattern: None,
    })
    .to_bytes()
}

#[tokio::test]
async fn phone_code_login_uses_the_sdk_wire_and_consumes_successful_challenges() {
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        move |_, bytes| {
            let attempt = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    let request = tl::functions::auth::SendCode::from_boxed_bytes(&bytes).unwrap();
                    assert_eq!(request.phone_number, "+15555550100");
                    assert_eq!(request.api_id, 1);
                    assert_eq!(request.api_hash, "0123456789abcdef0123456789abcdef");
                    Ok(sent_code())
                } else {
                    let request = tl::functions::auth::SignIn::from_boxed_bytes(&bytes).unwrap();
                    assert_eq!(request.phone_number, "+15555550100");
                    assert_eq!(request.phone_code_hash, "PRIVATE_CHALLENGE_HASH");
                    assert_eq!(request.phone_code.as_deref(), Some("12345"));
                    Ok(authorization(user(42, 1 | (1 << 10))))
                }
            }
        },
    );
    let challenge = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::Expected(AccountId::new(42).unwrap()),
            )
            .await
            .unwrap(),
    );
    assert!(!format!("{challenge:?}").contains("PRIVATE_CHALLENGE_HASH"));
    assert!(!format!("{challenge:?}").contains("15555550100"));
    assert_eq!(challenge.expected_length(), Some(5));
    assert_eq!(challenge.retry_after(), Some(Duration::from_secs(30)));
    assert!(
        matches!(client.submit_login_code(&challenge, LoginCode::new("12345").unwrap()).await.unwrap(), AuthStep::Authorized(identity) if identity==expected(false))
    );
    assert_eq!(session.remembered_account().unwrap(), Some(expected(false)));
    assert_eq!(
        client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    client.close().await.unwrap();
}

#[tokio::test]
async fn confirmed_invalid_code_can_retry_but_expired_code_consumes_the_capability() {
    for expired in [false, true] {
        let (_directory, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(session, ClientOptions::default(), move |_, _| {
            let attempt = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                match attempt {
                    0 => Ok(sent_code()),
                    1 => Err(rpc(
                        400,
                        if expired {
                            "PHONE_CODE_EXPIRED"
                        } else {
                            "PHONE_CODE_INVALID"
                        },
                    )),
                    _ => Ok(authorization(user(42, 1 | (1 << 10)))),
                }
            }
        });
        let challenge = code_challenge(
            client
                .begin_user_login(
                    credentials(),
                    PhoneNumber::new("+15555550100").unwrap(),
                    UserLoginTarget::NewAccount,
                )
                .await
                .unwrap(),
        );
        let failure = client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err();
        assert_eq!(failure.outcome(), Outcome::Rejected);
        assert_eq!(
            failure.kind(),
            if expired {
                ClientErrorKind::LoginCodeExpired
            } else {
                ClientErrorKind::InvalidLoginCode
            }
        );
        let retry = client
            .submit_login_code(&challenge, LoginCode::new("67890").unwrap())
            .await;
        if expired {
            assert_eq!(retry.unwrap_err().outcome(), Outcome::NotSent);
        } else {
            assert!(matches!(retry.unwrap(), AuthStep::Authorized(_)));
        }
        assert_eq!(calls.load(Ordering::SeqCst), if expired { 2 } else { 3 });
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn password_needed_consumes_code_before_a_failed_parameter_refresh() {
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        seen.fetch_add(1, Ordering::SeqCst);
        async move {
            if bytes[..4] == tl::functions::auth::SendCode::CONSTRUCTOR_ID.to_le_bytes() {
                Ok(sent_code())
            } else if bytes[..4] == tl::functions::auth::SignIn::CONSTRUCTOR_ID.to_le_bytes() {
                Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
            } else {
                assert_eq!(bytes, tl::functions::account::GetPassword {}.to_bytes());
                Err(rpc(400, "PRIVATE_REFRESH_ERROR"))
            }
        }
    });
    let challenge = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::Rejected
    );
    assert_eq!(
        client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}

#[tokio::test]
async fn two_factor_login_computes_maintained_srp_and_never_sends_the_password() {
    let (_directory, session) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen = checks.clone();
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        move |_, bytes| {
            let checks = seen.clone();
            async move {
                let constructor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if constructor == tl::functions::auth::SendCode::CONSTRUCTOR_ID {
                    Ok(sent_code())
                } else if constructor == tl::functions::auth::SignIn::CONSTRUCTOR_ID {
                    Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
                } else if constructor == tl::functions::account::GetPassword::CONSTRUCTOR_ID {
                    Ok(password_info(123))
                } else {
                    assert!(
                        !bytes
                            .windows(b"PRIVATE_PASSWORD".len())
                            .any(|part| part == b"PRIVATE_PASSWORD")
                    );
                    let request =
                        tl::functions::auth::CheckPassword::from_boxed_bytes(&bytes).unwrap();
                    let tl::enums::InputCheckPasswordSrp::Srp(proof) = request.password else {
                        panic!("expected maintained SRP proof");
                    };
                    assert_eq!(proof.srp_id, 123);
                    assert_eq!(proof.a.len(), 256);
                    assert_eq!(proof.m1.len(), 32);
                    assert!(proof.m1.iter().any(|value| *value != 0));
                    checks.fetch_add(1, Ordering::SeqCst);
                    Ok(authorization(user(42, 1 | (1 << 10))))
                }
            }
        },
    );
    let code = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    let AuthStep::PasswordRequired(password) = client
        .submit_login_code(&code, LoginCode::new("12345").unwrap())
        .await
        .unwrap()
    else {
        panic!("expected password challenge");
    };
    assert_eq!(password.reason(), PasswordPromptReason::Required);
    assert_eq!(password.hint(), Some("SYNTHETIC_PRIVATE_HINT"));
    assert!(!format!("{password:?}").contains("SYNTHETIC_PRIVATE_HINT"));
    assert!(
        matches!(client.submit_password(&password, Password::new("PRIVATE_PASSWORD").unwrap()).await.unwrap(), AuthStep::Authorized(identity) if identity==expected(false))
    );
    assert_eq!(session.remembered_account().unwrap(), Some(expected(false)));
    assert_eq!(
        client
            .submit_password(&password, Password::new("PRIVATE_PASSWORD").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    client.close().await.unwrap();
}

#[tokio::test]
async fn replaced_expired_and_cross_generation_capabilities_never_dispatch() {
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        seen.fetch_add(1, Ordering::SeqCst);
        async move {
            assert_eq!(
                &bytes[..4],
                &tl::functions::auth::SendCode::CONSTRUCTOR_ID.to_le_bytes()
            );
            Ok(sent_code())
        }
    });
    let first = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    let mut second = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        client
            .submit_login_code(&first, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    second.expire_for_test();
    assert_eq!(
        client
            .submit_login_code(&second, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    let (_other_dir, other_session) = super::session().await;
    let other = fixture(other_session, ClientOptions::default(), |_, _| async {
        panic!("foreign capabilities cannot reach a backend")
    });
    assert_eq!(
        other
            .submit_login_code(&first, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    other.close().await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn queued_duplicate_code_submissions_are_consumed_at_every_confirmed_stage() {
    for password_needed in [false, true] {
        let (_directory, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(session, ClientOptions::default(), move |_, bytes| {
            seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if bytes[..4] == tl::functions::auth::SendCode::CONSTRUCTOR_ID.to_le_bytes() {
                    Ok(sent_code())
                } else if bytes[..4]
                    == tl::functions::account::GetPassword::CONSTRUCTOR_ID.to_le_bytes()
                {
                    Ok(password_info(123))
                } else if password_needed {
                    Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
                } else {
                    Ok(authorization(user(42, 1 | (1 << 10))))
                }
            }
        });
        let challenge = code_challenge(
            client
                .begin_user_login(
                    credentials(),
                    PhoneNumber::new("+15555550100").unwrap(),
                    UserLoginTarget::Expected(AccountId::new(42).unwrap()),
                )
                .await
                .unwrap(),
        );
        let (first, second) = tokio::join!(
            client.submit_login_code(&challenge, LoginCode::new("12345").unwrap()),
            client.submit_login_code(&challenge, LoginCode::new("12345").unwrap())
        );
        let results = [first, second];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(error) if error.outcome()==Outcome::NotSent))
                .count(),
            1
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            if password_needed { 3 } else { 2 }
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn password_refusals_refresh_without_replaying_or_reusing_the_old_capability() {
    for expired in [false, true] {
        let (_directory, session) = session().await;
        let checks = Arc::new(AtomicUsize::new(0));
        let seen = checks.clone();
        let refreshes = Arc::new(AtomicUsize::new(0));
        let refreshed = refreshes.clone();
        let client = fixture(session, ClientOptions::default(), move |_, bytes| {
            let checks = seen.clone();
            let refreshes = refreshed.clone();
            async move {
                let constructor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if constructor == tl::functions::auth::SendCode::CONSTRUCTOR_ID {
                    Ok(sent_code())
                } else if constructor == tl::functions::auth::SignIn::CONSTRUCTOR_ID {
                    Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
                } else if constructor == tl::functions::account::GetPassword::CONSTRUCTOR_ID {
                    Ok(password_info(
                        123 + refreshes.fetch_add(1, Ordering::SeqCst) as i64,
                    ))
                } else {
                    checks.fetch_add(1, Ordering::SeqCst);
                    Err(rpc(
                        400,
                        if expired {
                            "SRP_ID_INVALID"
                        } else {
                            "PASSWORD_HASH_INVALID"
                        },
                    ))
                }
            }
        });
        let code = code_challenge(
            client
                .begin_user_login(
                    credentials(),
                    PhoneNumber::new("+15555550100").unwrap(),
                    UserLoginTarget::NewAccount,
                )
                .await
                .unwrap(),
        );
        let AuthStep::PasswordRequired(password) = client
            .submit_login_code(&code, LoginCode::new("12345").unwrap())
            .await
            .unwrap()
        else {
            panic!("expected 2FA");
        };
        let AuthStep::PasswordRequired(replacement) = client
            .submit_password(&password, Password::new("SYNTHETIC_PASSWORD").unwrap())
            .await
            .unwrap()
        else {
            panic!("expected explicit replacement");
        };
        assert_eq!(
            replacement.reason(),
            if expired {
                PasswordPromptReason::Expired
            } else {
                PasswordPromptReason::InvalidPassword
            }
        );
        assert_eq!(
            client
                .submit_password(&password, Password::new("SYNTHETIC_PASSWORD").unwrap())
                .await
                .unwrap_err()
                .outcome(),
            Outcome::NotSent
        );
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert_eq!(refreshes.load(Ordering::SeqCst), 2);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn one_action_cannot_redirect_twice_across_sign_in_and_parameter_refresh() {
    let (_directory, session) = session().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |dc, bytes| {
        let constructor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        observed.lock().unwrap().push((constructor, dc));
        async move {
            if constructor == tl::functions::auth::SendCode::CONSTRUCTOR_ID {
                Ok(sent_code())
            } else if constructor == tl::functions::auth::SignIn::CONSTRUCTOR_ID && dc == 3 {
                Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
            } else {
                Err(grammers_mtsender::InvocationError::Rpc(
                    grammers_mtsender::RpcError {
                        code: 303,
                        name: "USER_MIGRATE".into(),
                        value: Some(if dc == 2 { 3 } else { 4 }),
                        caused_by: None,
                    },
                ))
            }
        }
    });
    let challenge = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::Rejected
    );
    assert_eq!(
        client
            .submit_login_code(&challenge, LoginCode::new("12345").unwrap())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, dc)| *dc)
            .collect::<Vec<_>>(),
        vec![2, 2, 3, 3]
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn unsupported_payment_and_signup_are_explicit_and_new_targets_refuse_remembered_accounts() {
    use crate::auth::UnsupportedAuthFlow;
    for signup in [false, true] {
        let (_directory, session) = session().await;
        let client = fixture(session, ClientOptions::default(), move |_, _| async move {
            if signup {
                Ok(
                    tl::enums::auth::SentCode::Success(tl::types::auth::SentCodeSuccess {
                        authorization: tl::enums::auth::Authorization::SignUpRequired(
                            tl::types::auth::AuthorizationSignUpRequired {
                                terms_of_service: None,
                            },
                        ),
                    })
                    .to_bytes(),
                )
            } else {
                Ok(tl::enums::auth::SentCode::PaymentRequired(
                    tl::types::auth::SentCodePaymentRequired {
                        store_product: "PRIVATE_PRODUCT".into(),
                        phone_code_hash: "PRIVATE_HASH".into(),
                        support_email_address: "PRIVATE_EMAIL".into(),
                        support_email_subject: "PRIVATE_SUBJECT".into(),
                        premium_days: 1,
                        currency: "USD".into(),
                        amount: 1,
                    },
                )
                .to_bytes())
            }
        });
        let step = client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap();
        assert!(
            matches!(step, AuthStep::Unsupported(flow) if flow==if signup { UnsupportedAuthFlow::SignUp } else { UnsupportedAuthFlow::Payment })
        );
        assert!(!format!("{step:?}").contains("PRIVATE"));
        client.close().await.unwrap();
    }
    let (_directory, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        async { Ok(vec![user(42, 1 | (1 << 10))].to_bytes()) }
    });
    client.verify_account(expected(false)).await.unwrap();
    assert_eq!(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount
            )
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    client.close().await.unwrap();
}

#[tokio::test]
async fn malformed_srp_parameters_never_produce_a_password_check() {
    for mutation in 0..7 {
        let (_directory, session) = session().await;
        let client = fixture(
            session,
            ClientOptions::default(),
            move |_, bytes| async move {
                let constructor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if constructor == tl::functions::auth::SendCode::CONSTRUCTOR_ID {
                    return Ok(sent_code());
                }
                if constructor == tl::functions::auth::SignIn::CONSTRUCTOR_ID {
                    return Err(rpc(401, "SESSION_PASSWORD_NEEDED"));
                }
                assert_eq!(
                    constructor,
                    tl::functions::account::GetPassword::CONSTRUCTOR_ID
                );
                let tl::enums::account::Password::Password(mut info) =
                    tl::enums::account::Password::from_bytes(&password_info(123)).unwrap();
                let Some(
                    tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(
                        algo,
                    ),
                ) = info.current_algo.as_mut()
                else {
                    unreachable!();
                };
                match mutation {
                    0 => algo.g = 1,
                    1 => {
                        algo.p.pop();
                    }
                    2 => algo.p[0] = 0,
                    3 => info.srp_b = Some(vec![0; 256]),
                    4 => info.srp_b = Some(prime()),
                    5 => info.srp_b = Some(vec![0x5a; 257]),
                    _ => info.hint = Some("\u{1b}[PRIVATE_HINT".into()),
                }
                Ok(tl::enums::account::Password::Password(info).to_bytes())
            },
        );
        let code = code_challenge(
            client
                .begin_user_login(
                    credentials(),
                    PhoneNumber::new("+15555550100").unwrap(),
                    UserLoginTarget::NewAccount,
                )
                .await
                .unwrap(),
        );
        assert_eq!(
            client
                .submit_login_code(&code, LoginCode::new("12345").unwrap())
                .await
                .unwrap_err()
                .outcome(),
            Outcome::Unknown
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn real_srp_work_cannot_dispatch_after_its_original_deadline() {
    let (_directory, session) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen = checks.clone();
    let client = fixture(
        session,
        ClientOptions {
            before_compute: Some(Duration::from_millis(200)),
            ..ClientOptions::default().request_timeout(Duration::from_millis(100))
        },
        move |_, bytes| {
            let checks = seen.clone();
            async move {
                let constructor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if constructor == tl::functions::auth::SendCode::CONSTRUCTOR_ID {
                    Ok(sent_code())
                } else if constructor == tl::functions::auth::SignIn::CONSTRUCTOR_ID {
                    Err(rpc(401, "SESSION_PASSWORD_NEEDED"))
                } else if constructor == tl::functions::account::GetPassword::CONSTRUCTOR_ID {
                    Ok(password_info(123))
                } else {
                    checks.fetch_add(1, Ordering::SeqCst);
                    Ok(authorization(user(42, 1 | (1 << 10))))
                }
            }
        },
    );
    let code = code_challenge(
        client
            .begin_user_login(
                credentials(),
                PhoneNumber::new("+15555550100").unwrap(),
                UserLoginTarget::NewAccount,
            )
            .await
            .unwrap(),
    );
    let AuthStep::PasswordRequired(password) = client
        .submit_login_code(&code, LoginCode::new("12345").unwrap())
        .await
        .unwrap()
    else {
        panic!("expected 2FA");
    };
    let failure = client
        .submit_password(&password, Password::new("SYNTHETIC_PASSWORD").unwrap())
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Timeout);
    assert_eq!(failure.outcome(), Outcome::NotSent);
    client.close().await.unwrap();
    assert_eq!(checks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn epoch_exhaustion_never_wraps_or_dispatches_a_new_flow() {
    let (_directory, session) = session().await;
    let client = fixture(session, ClientOptions::default(), |_, _| async {
        panic!("exhausted capabilities cannot dispatch")
    });
    client.inner.auth_epoch.store(u64::MAX, Ordering::Release);
    let failure = client
        .begin_user_login(
            credentials(),
            PhoneNumber::new("+15555550100").unwrap(),
            UserLoginTarget::NewAccount,
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Capacity);
    assert_eq!(failure.outcome(), Outcome::NotSent);
    assert_eq!(client.inner.auth_epoch.load(Ordering::Acquire), u64::MAX);
    client.close().await.unwrap();
}
