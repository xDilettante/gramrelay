use super::*;
use crate::{AccountId, AccountIdentity, AccountKind, PeerId, SendText};
use std::sync::atomic::AtomicUsize;
#[path = "history_tests.rs"]
mod history;

fn expected() -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: AccountKind::User,
    }
}
fn user(id: i64, flags: u32, hash: i64) -> tl::enums::User {
    let mut bytes = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    if flags & 1 != 0 {
        bytes.extend_from_slice(&hash.to_le_bytes());
    }
    if flags & (1 << 14) != 0 {
        bytes.extend_from_slice(&0i32.to_le_bytes());
    }
    tl::enums::User::from_bytes(&bytes).unwrap()
}
fn self_reply(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes[..4] != tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes() {
        return None;
    }
    let request = tl::functions::users::GetUsers::from_bytes(&bytes[4..]).unwrap();
    assert!(matches!(
        request.id.as_slice(),
        [tl::enums::InputUser::UserSelf]
    ));
    Some(vec![user(42, 1 << 10, 0)].to_bytes())
}
fn resolved(users: Vec<tl::enums::User>, id: i64) -> Vec<u8> {
    tl::enums::contacts::ResolvedPeer::Peer(tl::types::contacts::ResolvedPeer {
        peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: id }),
        chats: vec![],
        users,
    })
    .to_bytes()
}

#[tokio::test]
async fn lookup_verifies_identity_and_persists_full_hash_without_exposing_it() {
    let (directory, session) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let calls = checks.clone();
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        move |_, bytes| {
            let calls = calls.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    calls.fetch_add(1, Ordering::SeqCst);
                    return Ok(reply);
                }
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                assert_eq!(
                    bytes[..4],
                    tl::functions::contacts::ResolveUsername::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request =
                    tl::functions::contacts::ResolveUsername::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.username, "example");
                assert!(request.referer.is_none());
                Ok(resolved(vec![user(77, 1, 987654321)], 77))
            }
        },
    );
    let account = client.account(expected());
    let peer = account.resolve_username("@example").await.unwrap();
    assert_eq!(peer.id(), PeerId::user(77).unwrap());
    assert!(!format!("{peer:?} {account:?}").contains("987654321"));
    let cached = account.cached_peer(peer.id()).await.unwrap().unwrap();
    assert_eq!(cached.id(), peer.id());
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    client.close().await.unwrap();

    let reopened = Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    let client = fixture(reopened, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).expect("cache lookup needs only self verification"))
    });
    assert!(
        client
            .account(expected())
            .cached_peer(peer.id())
            .await
            .unwrap()
            .is_some()
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn invalid_names_never_start_an_rpc_and_min_hash_never_grants_authority() {
    let (_directory, session) = session().await;
    let client = fixture(session, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes)
            .unwrap_or_else(|| resolved(vec![user(77, 1 | (1 << 20), 987654321)], 77)))
    });
    let account = client.account(expected());
    for name in ["", "@", "@@x", "a/b", "имя", "a b", &"a".repeat(33)] {
        assert_eq!(
            account.resolve_username(name).await.unwrap_err().outcome(),
            Outcome::NotSent
        );
    }
    let failure = account.resolve_username("example").await.unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::PeerUnavailable);
    assert!(
        account
            .cached_peer(PeerId::user(77).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    client.close().await.unwrap();
}

fn sent(id: i32) -> Vec<u8> {
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id,
        pts: 7,
        pts_count: 1,
        date: 123,
        media: None,
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}

#[tokio::test]
async fn text_send_uses_immutable_intent_and_verified_peer_with_no_paid_flags() {
    let (_directory, session) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let calls = checks.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                calls.fetch_add(1, Ordering::SeqCst);
                return Ok(reply);
            }
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                bytes[..4],
                tl::functions::messages::SendMessage::CONSTRUCTOR_ID.to_le_bytes()
            );
            let request = tl::functions::messages::SendMessage::from_bytes(&bytes[4..]).unwrap();
            assert!(matches!(request.peer, tl::enums::InputPeer::PeerSelf));
            assert_eq!(request.message, "private text");
            assert_eq!(request.random_id, 1234567);
            assert!(request.silent && request.no_webpage);
            assert!(!request.allow_paid_floodskip);
            assert!(
                request.allow_paid_stars.is_none()
                    && request.send_as.is_none()
                    && request.schedule_date.is_none()
            );
            Ok(sent(99))
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let intent = SendText::with_random_id("private text", 1234567)
        .unwrap()
        .silent(true);
    assert!(!format!("{intent:?}").contains("private text"));
    assert_eq!(intent.random_id(), 1234567);
    let receipt = account.send_text(&peer, intent).await.unwrap();
    assert_eq!(receipt.peer(), peer.id());
    assert_eq!(receipt.message_id(), 99);
    assert_eq!(receipt.date(), 123);
    client.close().await.unwrap();
}

#[tokio::test]
async fn peer_references_refuse_other_client_account_and_authorization_epoch() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).expect("no data RPC is allowed"))
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let (_other_directory, other_session) = session().await;
    let other = fixture(other_session, ClientOptions::default(), |_, _| async {
        panic!("foreign reference must fail before RPC")
    });
    let failure = other
        .account(expected())
        .send_text(&peer, SendText::new("hello").unwrap())
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::InvalidPeer);
    let wrong = AccountIdentity {
        id: AccountId::new(43).unwrap(),
        ..expected()
    };
    assert_eq!(
        client
            .account(wrong)
            .send_text(&peer, SendText::new("hello").unwrap())
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidPeer
    );
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    assert_eq!(
        account
            .send_text(&peer, SendText::new("hello").unwrap())
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidPeer
    );
    other.close().await.unwrap();
    client.close().await.unwrap();
}

#[test]
fn text_intents_validate_before_dispatch_and_keep_generated_ids_stable() {
    for text in ["", "a\0b", &"x".repeat(4097), &"😀".repeat(2049)] {
        assert_eq!(SendText::new(text).unwrap_err().outcome(), Outcome::NotSent);
    }
    assert!(SendText::with_random_id("valid", 0).is_err());
    let intent = SendText::new("😀".repeat(2048)).unwrap();
    assert_ne!(intent.random_id(), 0);
    assert_eq!(intent.random_id(), intent.random_id());
}

#[tokio::test]
async fn contradictory_expected_self_record_rejects_before_any_target_cache_commit() {
    let (directory, stored) = session().await;
    let client = fixture(
        stored.clone(),
        ClientOptions::default(),
        |_, bytes| async move {
            Ok(self_reply(&bytes).unwrap_or_else(|| {
                resolved(vec![user(77, 1, 987654321), user(42, 1 << 14, 0)], 77)
            }))
        },
    );
    let failure = client
        .account(expected())
        .resolve_username("example")
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::IdentityMismatch);
    client.close().await.unwrap();
    let reopened = Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        reopened
            .cached_peer(grammers_session::types::PeerId::user(77).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    reopened.close().await.unwrap();
}

fn message(id: i32, peer: i64, text: &str, out: bool) -> tl::enums::Message {
    let mut bytes = tl::types::Message::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&if out { 2u32 } else { 0 }.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(
        &tl::enums::Peer::User(tl::types::PeerUser { user_id: peer }).to_bytes(),
    );
    bytes.extend_from_slice(&123i32.to_le_bytes());
    bytes.extend_from_slice(&text.to_owned().to_bytes());
    tl::enums::Message::from_bytes(&bytes).unwrap()
}
fn new_message(id: i32, peer: i64, text: &str, out: bool) -> tl::enums::Update {
    tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
        message: message(id, peer, text, out),
        pts: 7,
        pts_count: 1,
    })
}
fn mapped(id: i32, random_id: i64) -> tl::enums::Update {
    tl::enums::Update::MessageId(tl::types::UpdateMessageId { id, random_id })
}
fn updates(updates: Vec<tl::enums::Update>) -> Vec<u8> {
    tl::enums::Updates::Updates(tl::types::Updates {
        updates,
        users: vec![],
        chats: vec![],
        date: 123,
        seq: 3,
    })
    .to_bytes()
}

#[tokio::test]
async fn receipts_require_exact_mapping_peer_text_and_unique_message_evidence() {
    let valid = vec![mapped(99, 1234567), new_message(99, 42, "hello", true)];
    let cases = vec![
        (updates(valid.clone()), true),
        (updates(vec![new_message(99, 42, "hello", true)]), false),
        (
            updates(vec![
                mapped(99, 7654321),
                new_message(99, 42, "hello", true),
            ]),
            false,
        ),
        (
            updates(vec![
                mapped(99, 1234567),
                new_message(99, 43, "hello", true),
            ]),
            false,
        ),
        (
            updates(vec![
                mapped(99, 1234567),
                new_message(99, 42, "wrong", true),
            ]),
            false,
        ),
        (
            updates(vec![
                mapped(99, 1234567),
                new_message(99, 42, "hello", false),
            ]),
            false,
        ),
        (
            updates(vec![
                mapped(99, 1234567),
                mapped(99, 1234567),
                new_message(99, 42, "hello", true),
            ]),
            false,
        ),
        (
            updates(vec![
                mapped(99, 1234567),
                new_message(99, 42, "hello", true),
                new_message(99, 42, "hello", true),
            ]),
            false,
        ),
        (tl::enums::Updates::TooLong.to_bytes(), false),
        (sent(0), false),
    ];
    for (response, success) in cases {
        let (_directory, session) = session().await;
        let writes = Arc::new(AtomicUsize::new(0));
        let calls = writes.clone();
        let client = fixture(session, ClientOptions::default(), move |_, bytes| {
            let calls = calls.clone();
            let response = response.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMessage::CONSTRUCTOR_ID.to_le_bytes()
                );
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(response)
            }
        });
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        let result = account
            .send_text(&peer, SendText::with_random_id("hello", 1234567).unwrap())
            .await;
        if success {
            assert_eq!(result.unwrap().message_id(), 99);
        } else {
            let failure = result.unwrap_err();
            assert_eq!(failure.kind(), ClientErrorKind::Decode);
            assert_eq!(failure.outcome(), Outcome::Unknown);
        }
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn edits_require_an_actual_matching_edit_response() {
    for valid in [true, false] {
        let (_directory, session) = session().await;
        let client = fixture(
            session,
            ClientOptions::default(),
            move |_, bytes| async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::EditMessage::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request =
                    tl::functions::messages::EditMessage::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.id, 99);
                assert_eq!(request.message.as_deref(), Some("changed"));
                assert!(request.no_webpage);
                assert!(
                    request.media.is_none()
                        && request.reply_markup.is_none()
                        && request.schedule_date.is_none()
                );
                let update = if valid {
                    tl::enums::Update::EditMessage(tl::types::UpdateEditMessage {
                        message: message(99, 42, "changed", false),
                        pts: 8,
                        pts_count: 1,
                    })
                } else {
                    new_message(99, 42, "changed", true)
                };
                Ok(updates(vec![update]))
            },
        );
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            account
                .edit_text(&peer, 0, "changed")
                .await
                .unwrap_err()
                .outcome(),
            Outcome::NotSent
        );
        let result = account.edit_text(&peer, 99, "changed").await;
        if valid {
            assert_eq!(result.unwrap().message_id(), 99);
        } else {
            assert_eq!(result.unwrap_err().outcome(), Outcome::Unknown);
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn min_and_empty_responses_preserve_a_previously_verified_full_hash() {
    let (_directory, stored) = session().await;
    let lookups = Arc::new(AtomicUsize::new(0));
    let calls = lookups.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::contacts::ResolveUsername::CONSTRUCTOR_ID.to_le_bytes()
            {
                let index = calls.fetch_add(1, Ordering::SeqCst);
                return Ok(resolved(
                    vec![match index {
                        0 => user(77, 1, 987654321),
                        1 => user(77, 1 | (1 << 20), 123456789),
                        _ => tl::enums::User::Empty(tl::types::UserEmpty { id: 77 }),
                    }],
                    77,
                ));
            }
            let request = tl::functions::messages::SendMessage::from_bytes(&bytes[4..]).unwrap();
            assert!(
                matches!(request.peer, tl::enums::InputPeer::User(input) if input.user_id == 77 && input.access_hash == 987654321)
            );
            Ok(sent(99))
        }
    });
    let account = client.account(expected());
    for _ in 0..3 {
        let peer = account.resolve_username("example").await.unwrap();
        account
            .send_text(&peer, SendText::new("hello").unwrap())
            .await
            .unwrap();
    }
    assert_eq!(lookups.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}

#[tokio::test]
async fn invalid_and_duplicate_entities_fail_closed() {
    let cases = vec![
        resolved(vec![user(77, 1, 1), user(77, 1, 2)], 77),
        resolved(vec![user(-1, 1, 1)], -1),
        resolved(vec![user(77, 1 | (1 << 10), 1)], 77),
        resolved(vec![user(77, 1, 1)], 0),
        resolved((1..=1025).map(|id| user(1000 + id, 1, 1)).collect(), 1001),
    ];
    for response in cases {
        let (_directory, stored) = session().await;
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let response = response.clone();
            async move { Ok(self_reply(&bytes).unwrap_or(response)) }
        });
        assert_eq!(
            client
                .account(expected())
                .resolve_username("example")
                .await
                .unwrap_err()
                .outcome(),
            Outcome::Unknown
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn timed_out_send_is_unknown_never_replayed_and_joins_backend() {
    let (_directory, stored) = session().await;
    let writes = Arc::new(AtomicUsize::new(0));
    let calls = writes.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let status = stopped.clone();
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let client = fixture(
        stored,
        ClientOptions::default().request_timeout(Duration::from_millis(100)),
        move |_, bytes| {
            let calls = calls.clone();
            let status = status.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMessage::CONSTRUCTOR_ID.to_le_bytes()
                );
                calls.fetch_add(1, Ordering::SeqCst);
                let _stop = Stop(status);
                std::future::pending().await
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let failure = account
        .send_text(&peer, SendText::new("hello").unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::Timeout, Outcome::Unknown)
    );
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    client.close().await.unwrap();
    assert!(stopped.load(Ordering::Acquire));
}

#[tokio::test]
async fn conflicting_empty_records_and_negative_update_counters_are_not_receipts() {
    let empty = tl::enums::Message::Empty(tl::types::MessageEmpty {
        id: 99,
        peer_id: Some(tl::enums::Peer::User(tl::types::PeerUser { user_id: 42 })),
    });
    for edit in [false, true] {
        for negative_counter in [false, true] {
            let (_directory, stored) = session().await;
            let empty = empty.clone();
            let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
                let empty = empty.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    let mut evidence = vec![mapped(99, 1234567)];
                    if edit {
                        evidence.push(tl::enums::Update::EditMessage(
                            tl::types::UpdateEditMessage {
                                message: message(99, 42, "hello", true),
                                pts: if negative_counter { -1 } else { 7 },
                                pts_count: 1,
                            },
                        ));
                        if !negative_counter {
                            evidence.push(tl::enums::Update::EditMessage(
                                tl::types::UpdateEditMessage {
                                    message: empty,
                                    pts: 7,
                                    pts_count: 1,
                                },
                            ));
                        }
                    } else {
                        evidence.push(tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
                            message: message(99, 42, "hello", true),
                            pts: 7,
                            pts_count: if negative_counter { -1 } else { 1 },
                        }));
                        if !negative_counter {
                            evidence.push(tl::enums::Update::NewMessage(
                                tl::types::UpdateNewMessage {
                                    message: empty,
                                    pts: 7,
                                    pts_count: 1,
                                },
                            ));
                        }
                    }
                    Ok(updates(evidence))
                }
            });
            let account = client.account(expected());
            let peer = account
                .cached_peer(PeerId::user(42).unwrap())
                .await
                .unwrap()
                .unwrap();
            let result = if edit {
                account.edit_text(&peer, 99, "hello").await
            } else {
                account
                    .send_text(&peer, SendText::with_random_id("hello", 1234567).unwrap())
                    .await
            };
            assert_eq!(result.unwrap_err().outcome(), Outcome::Unknown);
            client.close().await.unwrap();
        }
    }
}

#[tokio::test]
async fn queued_peer_is_checked_again_after_authorization_guard() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).expect("changed epoch must prevent a data RPC"))
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let gate = client.inner.authorization.clone().write_owned().await;
    let call = tokio::spawn(async move {
        account
            .send_text(&peer, SendText::new("hello").unwrap())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.inner.slots.available_permits() == 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    drop(gate);
    let failure = call.await.unwrap().unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::InvalidPeer, Outcome::NotSent)
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn dropping_last_host_handle_stops_cancelled_lookup_without_waiting_for_deadline() {
    let (_directory, stored) = session().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let signal = signal.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            signal.notify_one();
            std::future::pending().await
        }
    });
    let mut exited = client.inner.shared.exited.subscribe();
    let account = client.account(expected());
    let mut call = Box::pin(account.resolve_username("example"));
    tokio::select! {
        result = &mut call => panic!("lookup unexpectedly completed: {result:?}"),
        _ = started.notified() => {}
    }
    drop(call);
    drop(account);
    drop(client);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(result) = *exited.borrow_and_update() {
                break result;
            }
            exited.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn common_actions_do_not_advance_provisional_update_state() {
    use grammers_session::{Session as _, types::UpdateState};
    let (_directory, stored) = session().await;
    let claim = stored.claim_client().unwrap();
    let backend = claim.backend();
    drop(claim);
    backend
        .set_update_state(UpdateState::Primary {
            pts: 100,
            date: 50,
            seq: 3,
        })
        .await
        .unwrap();
    let before = backend.updates_state().await.unwrap();
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        if bytes[..4] == tl::functions::contacts::ResolveUsername::CONSTRUCTOR_ID.to_le_bytes() {
            return Ok(resolved(vec![user(77, 1, 987654321)], 77));
        }
        Ok(sent(99))
    });
    let account = client.account(expected());
    let peer = account.resolve_username("example").await.unwrap();
    account
        .send_text(&peer, SendText::new("hello").unwrap())
        .await
        .unwrap();
    assert_eq!(backend.updates_state().await.unwrap(), before);
    client.close().await.unwrap();
}

#[tokio::test]
async fn cached_chat_and_channel_use_distinct_routing_without_implicit_hashes() {
    use grammers_session::types::{PeerAuth, PeerInfo};
    let (_directory, stored) = session().await;
    stored
        .cache_peers(vec![
            PeerInfo::Chat { id: 77 },
            PeerInfo::Channel {
                id: 77,
                auth: Some(PeerAuth::from_hash(987654321)),
                kind: None,
            },
            PeerInfo::Channel {
                id: 78,
                auth: None,
                kind: None,
            },
        ])
        .await
        .unwrap();
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        let request = tl::functions::messages::SendMessage::from_bytes(&bytes[4..]).unwrap();
        match request.peer {
            tl::enums::InputPeer::Chat(peer) => assert_eq!(peer.chat_id, 77),
            tl::enums::InputPeer::Channel(peer) => {
                assert_eq!((peer.channel_id, peer.access_hash), (77, 987654321))
            }
            other => panic!("unexpected routing kind: {other:?}"),
        }
        Ok(sent(99))
    });
    let account = client.account(expected());
    let chat = account
        .cached_peer(PeerId::chat(77).unwrap())
        .await
        .unwrap()
        .unwrap();
    let channel = account
        .cached_peer(PeerId::channel(77).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(chat.id(), channel.id());
    assert!(
        account
            .cached_peer(PeerId::channel(78).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    for peer in [chat, channel] {
        account
            .send_text(&peer, SendText::new("hello").unwrap())
            .await
            .unwrap();
    }
    client.close().await.unwrap();
}
