use super::*;
use crate::HistoryMessageKind;

fn history_response(messages: Vec<tl::enums::Message>, complete: bool, inexact: bool) -> Vec<u8> {
    if complete {
        tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
            messages,
            topics: vec![],
            users: vec![],
            chats: vec![],
        })
        .to_bytes()
    } else {
        tl::enums::messages::Messages::Slice(tl::types::messages::MessagesSlice {
            inexact,
            count: 100,
            next_rate: None,
            offset_id_offset: None,
            search_flood: None,
            messages,
            topics: vec![],
            users: vec![],
            chats: vec![],
        })
        .to_bytes()
    }
}

#[tokio::test]
async fn short_history_pages_continue_with_exclusive_immutable_cursors() {
    let (_directory, stored) = session().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            assert_eq!(
                bytes[..4],
                tl::functions::messages::GetHistory::CONSTRUCTOR_ID.to_le_bytes()
            );
            let request = tl::functions::messages::GetHistory::from_bytes(&bytes[4..]).unwrap();
            assert!(matches!(request.peer, tl::enums::InputPeer::PeerSelf));
            assert_eq!(request.limit, 10);
            assert_eq!(
                (
                    request.offset_date,
                    request.add_offset,
                    request.max_id,
                    request.min_id,
                    request.hash
                ),
                (0, 0, 0, 0, 0)
            );
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert_eq!(request.offset_id, 0);
                    Ok(history_response(
                        vec![
                            message(30, 42, "private content", false),
                            message(25, 42, "older", true),
                        ],
                        false,
                        false,
                    ))
                }
                1 => {
                    assert_eq!(request.offset_id, 25);
                    Ok(history_response(
                        vec![message(20, 42, "oldest", false)],
                        false,
                        false,
                    ))
                }
                _ => {
                    assert_eq!(request.offset_id, 20);
                    Ok(history_response(vec![], false, false))
                }
            }
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let first = account.history(&peer, 10, None).await.unwrap();
    assert_eq!(first.messages().len(), 2);
    let newest = &first.messages()[0];
    assert_eq!(newest.id(), 30);
    assert_eq!(newest.peer(), peer.id());
    assert_eq!(newest.kind(), HistoryMessageKind::Regular);
    assert_eq!(newest.text(), Some("private content"));
    assert_eq!(newest.date(), Some(123));
    assert_eq!(newest.is_outgoing(), Some(false));
    assert_eq!(first.total_count(), Some(100));
    assert!(!first.is_inexact());
    assert!(!format!("{first:?} {:?}", first.next_cursor()).contains("private content"));
    let second = account
        .history(&peer, 10, first.next_cursor())
        .await
        .unwrap();
    assert_eq!(second.messages()[0].id(), 20);
    let third = account
        .history(&peer, 10, second.next_cursor())
        .await
        .unwrap();
    assert!(third.messages().is_empty() && third.next_cursor().is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}

#[tokio::test]
async fn full_history_response_is_terminal_without_hiding_empty_and_service_records() {
    let mut service = tl::types::MessageService::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    service.extend_from_slice(&0u32.to_le_bytes());
    service.extend_from_slice(&2i32.to_le_bytes());
    service
        .extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 42 }).to_bytes());
    service.extend_from_slice(&123i32.to_le_bytes());
    service.extend_from_slice(&tl::enums::MessageAction::Empty.to_bytes());
    let service = tl::enums::Message::from_bytes(&service).unwrap();
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let service = service.clone();
        async move {
            Ok(self_reply(&bytes).unwrap_or_else(|| {
                history_response(
                    vec![
                        message(3, 42, "text", false),
                        service,
                        tl::enums::Message::Empty(tl::types::MessageEmpty {
                            id: 1,
                            peer_id: None,
                        }),
                    ],
                    true,
                    false,
                )
            }))
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let page = account.history(&peer, 3, None).await.unwrap();
    assert!(page.next_cursor().is_none());
    assert_eq!(page.total_count(), None);
    assert_eq!(
        page.messages().iter().map(|m| m.kind()).collect::<Vec<_>>(),
        vec![
            HistoryMessageKind::Regular,
            HistoryMessageKind::Service,
            HistoryMessageKind::Empty
        ]
    );
    assert!(page.messages()[1].text().is_none());
    assert!(page.messages()[2].date().is_none());
    assert_eq!(page.messages()[2].peer(), peer.id());
    assert!(matches!(
        page.messages()[2].raw(),
        tl::enums::Message::Empty(_)
    ));
    client.close().await.unwrap();
}

fn slice(
    messages: Vec<tl::enums::Message>,
    count: i32,
    inexact: bool,
    position: Option<i32>,
) -> Vec<u8> {
    tl::enums::messages::Messages::Slice(tl::types::messages::MessagesSlice {
        inexact,
        count,
        next_rate: None,
        offset_id_offset: position,
        search_flood: None,
        messages,
        topics: vec![],
        users: vec![],
        chats: vec![],
    })
    .to_bytes()
}

#[tokio::test]
async fn malformed_history_never_becomes_a_successful_empty_or_wrong_scope_page() {
    let mut bad_date = message(10, 42, "text", false);
    if let tl::enums::Message::Message(value) = &mut bad_date {
        value.date = 0;
    }
    let mut bad_sender = message(10, 42, "text", false);
    if let tl::enums::Message::Message(value) = &mut bad_sender {
        value.from_id = Some(tl::enums::Peer::User(tl::types::PeerUser { user_id: -1 }));
    }
    let cases = vec![
        (
            history_response(vec![message(10, 43, "wrong peer", false)], true, false),
            ClientErrorKind::Decode,
        ),
        (
            history_response(vec![message(0, 42, "zero id", false)], true, false),
            ClientErrorKind::Decode,
        ),
        (
            history_response(
                vec![message(10, 42, "dup", false), message(10, 42, "dup", false)],
                true,
                false,
            ),
            ClientErrorKind::Decode,
        ),
        (
            history_response(
                vec![
                    message(9, 42, "wrong order", false),
                    message(10, 42, "wrong order", false),
                ],
                true,
                false,
            ),
            ClientErrorKind::Decode,
        ),
        (
            history_response(
                vec![
                    message(10, 42, "too many", false),
                    message(9, 42, "too many", false),
                    message(8, 42, "too many", false),
                ],
                true,
                false,
            ),
            ClientErrorKind::Decode,
        ),
        (
            history_response(vec![bad_date], true, false),
            ClientErrorKind::Decode,
        ),
        (
            history_response(vec![bad_sender], true, false),
            ClientErrorKind::Decode,
        ),
        (
            history_response(
                vec![message(10, 42, &"x".repeat(65537), false)],
                true,
                false,
            ),
            ClientErrorKind::Decode,
        ),
        (
            slice(vec![message(10, 42, "bad count", false)], -1, false, None),
            ClientErrorKind::Decode,
        ),
        (
            slice(
                vec![message(10, 42, "bad position", false)],
                10,
                false,
                Some(-1),
            ),
            ClientErrorKind::Decode,
        ),
        (
            slice(vec![message(10, 42, "bad total", false)], 0, false, None),
            ClientErrorKind::Decode,
        ),
        (
            slice(vec![], 100, false, None),
            ClientErrorKind::IncompletePage,
        ),
        (
            slice(vec![], 0, true, None),
            ClientErrorKind::IncompletePage,
        ),
        (
            tl::enums::messages::Messages::NotModified(tl::types::messages::MessagesNotModified {
                count: 0,
            })
            .to_bytes(),
            ClientErrorKind::Decode,
        ),
        (
            tl::enums::messages::Messages::ChannelMessages(tl::types::messages::ChannelMessages {
                inexact: false,
                pts: 10,
                count: 1,
                offset_id_offset: None,
                messages: vec![message(10, 42, "wrong response scope", false)],
                topics: vec![],
                chats: vec![],
                users: vec![],
            })
            .to_bytes(),
            ClientErrorKind::Decode,
        ),
    ];
    for (response, kind) in cases {
        let (_directory, stored) = session().await;
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let response = response.clone();
            async move { Ok(self_reply(&bytes).unwrap_or(response)) }
        });
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        let failure = account.history(&peer, 2, None).await.unwrap_err();
        assert_eq!(
            (failure.kind(), failure.outcome()),
            (kind, Outcome::Unknown)
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn repeated_boundary_record_is_rejected_instead_of_minting_a_loop_cursor() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).unwrap_or_else(|| {
            history_response(vec![message(10, 42, "boundary", false)], false, false)
        }))
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let page = account.history(&peer, 2, None).await.unwrap();
    let failure = account
        .history(&peer, 2, page.next_cursor())
        .await
        .unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::Decode, Outcome::Unknown)
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn cursor_binding_refuses_wrong_peer_other_client_and_prior_auth_epoch() {
    use grammers_session::types::{PeerAuth, PeerInfo};
    let (_directory, stored) = session().await;
    stored
        .cache_peers(vec![PeerInfo::User {
            id: 77,
            auth: Some(PeerAuth::from_hash(123)),
            bot: Some(false),
            is_self: Some(false),
        }])
        .await
        .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(history_response(
                vec![message(10, 42, "cursor", false)],
                false,
                false,
            ))
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let other_peer = account
        .cached_peer(PeerId::user(77).unwrap())
        .await
        .unwrap()
        .unwrap();
    let first = account.history(&peer, 1, None).await.unwrap();
    let cursor = first.next_cursor().unwrap().clone();
    assert_eq!(
        account
            .history(&other_peer, 1, Some(&cursor))
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidCursor
    );

    let (_directory2, stored2) = session().await;
    let other = fixture(stored2, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).expect("foreign cursor must not reach data RPC"))
    });
    let other_account = other.account(expected());
    let other_self = other_account.cached_peer(peer.id()).await.unwrap().unwrap();
    let failure = other_account
        .history(&other_self, 1, Some(&cursor))
        .await
        .unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::InvalidCursor, Outcome::NotSent)
    );
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    let fresh_self = account.cached_peer(peer.id()).await.unwrap().unwrap();
    assert_eq!(
        account
            .history(&fresh_self, 1, Some(&cursor))
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidCursor
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    other.close().await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn queued_history_rechecks_epoch_after_the_authorization_guard() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).expect("obsolete peer cannot read history"))
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let gate = client.inner.authorization.clone().write_owned().await;
    let pending = tokio::spawn(async move { account.history(&peer, 1, None).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.inner.slots.available_permits() == 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    drop(gate);
    assert_eq!(
        pending.await.unwrap().unwrap_err().outcome(),
        Outcome::NotSent
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn user_only_and_page_size_checks_do_not_start_a_history_rpc() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        assert_eq!(
            bytes[..4],
            tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes()
        );
        Ok(vec![user(42, (1 << 10) | (1 << 14), 0)].to_bytes())
    });
    let account = client.account(AccountIdentity {
        kind: AccountKind::Bot,
        ..expected()
    });
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    for limit in [0, 101] {
        assert_eq!(
            account
                .history(&peer, limit, None)
                .await
                .unwrap_err()
                .kind(),
            ClientErrorKind::InvalidInput
        );
    }
    let failure = account.history(&peer, 1, None).await.unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::UnsupportedOperation, Outcome::NotSent)
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn changed_live_identity_blocks_history_before_any_data_rpc() {
    let (_directory, stored) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let calls = checks.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            assert_eq!(
                bytes[..4],
                tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes()
            );
            let id = if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                42
            } else {
                43
            };
            Ok(vec![user(id, 1 << 10, 0)].to_bytes())
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let failure = account.history(&peer, 1, None).await.unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::IdentityMismatch);
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    client.close().await.unwrap();
}

#[tokio::test]
async fn channel_history_exposes_inexactness_and_caches_peers_without_advancing_pts() {
    use grammers_session::{
        Session as _,
        types::{PeerAuth, PeerInfo, UpdateState},
    };
    let (_directory, stored) = session().await;
    stored
        .cache_peers(vec![PeerInfo::Channel {
            id: 77,
            auth: Some(PeerAuth::from_hash(123)),
            kind: None,
        }])
        .await
        .unwrap();
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
    let client = fixture(
        stored.clone(),
        ClientOptions::default(),
        |_, bytes| async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            let request = tl::functions::messages::GetHistory::from_bytes(&bytes[4..]).unwrap();
            assert!(
                matches!(request.peer, tl::enums::InputPeer::Channel(peer) if (peer.channel_id, peer.access_hash) == (77, 123))
            );
            let mut raw = message(10, 42, "caption", false);
            if let tl::enums::Message::Message(value) = &mut raw {
                value.peer_id = tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 77 });
            }
            Ok(tl::enums::messages::Messages::ChannelMessages(
                tl::types::messages::ChannelMessages {
                    inexact: true,
                    pts: 999,
                    count: 0,
                    offset_id_offset: Some(0),
                    messages: vec![raw],
                    topics: vec![],
                    chats: vec![],
                    users: vec![user(88, 1, 987654321)],
                },
            )
            .to_bytes())
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::channel(77).unwrap())
        .await
        .unwrap()
        .unwrap();
    let page = account.history(&peer, 2, None).await.unwrap();
    assert!(page.is_inexact() && page.next_cursor().is_some());
    assert_eq!(page.total_count(), Some(0));
    assert_eq!(backend.updates_state().await.unwrap(), before);
    let cached = stored
        .cached_peer(grammers_session::types::PeerId::user(88).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cached.auth().unwrap().hash(), 987654321);
    client.close().await.unwrap();
}

#[tokio::test]
async fn timed_out_history_is_not_replayed_and_close_joins_the_pending_rpc() {
    let (_directory, stored) = session().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
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
    let failure = account.history(&peer, 10, None).await.unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::Timeout, Outcome::Unknown)
    );
    client.close().await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(stopped.load(Ordering::Acquire));
}
