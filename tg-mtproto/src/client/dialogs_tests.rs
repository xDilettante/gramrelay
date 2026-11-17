use super::*;
use crate::{DialogContinuation, DialogFolder, DialogId};

fn dialog(id: i64, top: i32, pinned: bool, folder: i32) -> tl::enums::Dialog {
    let mut bytes = tl::types::Dialog::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    let flags = if pinned { 4u32 } else { 0 } | if folder != 0 { 16 } else { 0 };
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: id }).to_bytes());
    for value in [top, 0, 0, 2, 0, 0, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&tl::types::PeerNotifySettings::CONSTRUCTOR_ID.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    if folder != 0 {
        bytes.extend_from_slice(&folder.to_le_bytes());
    }
    tl::enums::Dialog::from_bytes(&bytes).unwrap()
}
fn dated(id: i32, peer: i64, date: i32) -> tl::enums::Message {
    let mut raw = message(id, peer, "private last message", false);
    if let tl::enums::Message::Message(raw) = &mut raw {
        raw.date = date;
    }
    raw
}
fn pinned(
    dialogs: Vec<tl::enums::Dialog>,
    messages: Vec<tl::enums::Message>,
    users: Vec<tl::enums::User>,
) -> Vec<u8> {
    tl::enums::messages::PeerDialogs::Dialogs(tl::types::messages::PeerDialogs {
        dialogs,
        messages,
        users,
        chats: vec![],
        state: tl::enums::updates::State::State(tl::types::updates::State {
            pts: 900,
            qts: 0,
            date: 999,
            seq: 2,
            unread_count: 0,
        }),
    })
    .to_bytes()
}
fn ordinary(
    dialogs: Vec<tl::enums::Dialog>,
    messages: Vec<tl::enums::Message>,
    users: Vec<tl::enums::User>,
    complete: bool,
) -> Vec<u8> {
    if complete {
        tl::enums::messages::Dialogs::Dialogs(tl::types::messages::Dialogs {
            dialogs,
            messages,
            users,
            chats: vec![],
        })
        .to_bytes()
    } else {
        tl::enums::messages::Dialogs::Slice(tl::types::messages::DialogsSlice {
            count: 100,
            dialogs,
            messages,
            users,
            chats: vec![],
        })
        .to_bytes()
    }
}

#[tokio::test]
async fn pinned_snapshot_pages_cannot_shift_the_ordinary_offset_or_repeat_the_pinned_rpc() {
    let (_directory, stored) = session().await;
    let pin_calls = Arc::new(AtomicUsize::new(0));
    let normal_calls = Arc::new(AtomicUsize::new(0));
    let pins = pin_calls.clone();
    let normals = normal_calls.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let pins = pins.clone();
        let normals = normals.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
            {
                assert_eq!(pins.fetch_add(1, Ordering::SeqCst), 0);
                let request =
                    tl::functions::messages::GetPinnedDialogs::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.folder_id, 0);
                return Ok(pinned(
                    vec![dialog(77, 5, true, 0), dialog(78, 6, true, 0)],
                    vec![dated(5, 77, 100), dated(6, 78, 50)],
                    vec![user(77, 1, 123), user(78, 1, 456)],
                ));
            }
            assert_eq!(
                bytes[..4],
                tl::functions::messages::GetDialogs::CONSTRUCTOR_ID.to_le_bytes()
            );
            let request = tl::functions::messages::GetDialogs::from_bytes(&bytes[4..]).unwrap();
            assert!(request.exclude_pinned);
            assert_eq!(
                (request.folder_id, request.limit, request.hash),
                (Some(0), 2, 0)
            );
            match normals.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert_eq!((request.offset_date, request.offset_id), (0, 0));
                    assert!(matches!(request.offset_peer, tl::enums::InputPeer::Empty));
                    Ok(ordinary(
                        vec![dialog(79, 90, false, 0)],
                        vec![dated(90, 79, 200)],
                        vec![user(79, 1, 789)],
                        false,
                    ))
                }
                _ => {
                    assert_eq!((request.offset_date, request.offset_id), (200, 90));
                    assert!(
                        matches!(request.offset_peer, tl::enums::InputPeer::User(peer) if (peer.user_id, peer.access_hash) == (79, 789))
                    );
                    Ok(ordinary(vec![], vec![], vec![], true))
                }
            }
        }
    });
    let account = client.account(expected());
    let first = account.dialogs(DialogFolder::Main, 1, None).await.unwrap();
    assert_eq!(
        first.dialogs()[0].id(),
        DialogId::Peer(PeerId::user(77).unwrap())
    );
    assert!(first.dialogs()[0].is_pinned());
    assert_eq!(first.dialogs()[0].last_message().unwrap().id(), 5);
    assert!(!format!("{first:?}").contains("private last message"));
    let DialogContinuation::More(cursor) = first.continuation() else {
        panic!("expected second pinned page")
    };
    let second = account
        .dialogs(DialogFolder::Main, 1, Some(cursor))
        .await
        .unwrap();
    assert_eq!(
        second.dialogs().as_ptr(),
        first.dialogs().as_ptr().wrapping_add(1)
    );
    assert_eq!(
        second.dialogs()[0].id(),
        DialogId::Peer(PeerId::user(78).unwrap())
    );
    let DialogContinuation::More(cursor) = second.continuation() else {
        panic!("expected ordinary phase")
    };
    let third = account
        .dialogs(DialogFolder::Main, 2, Some(cursor))
        .await
        .unwrap();
    assert_eq!(
        third.dialogs()[0].id(),
        DialogId::Peer(PeerId::user(79).unwrap())
    );
    let DialogContinuation::More(cursor) = third.continuation() else {
        panic!("short slice must continue")
    };
    let fourth = account
        .dialogs(DialogFolder::Main, 2, Some(cursor))
        .await
        .unwrap();
    assert!(fourth.dialogs().is_empty());
    assert!(matches!(
        fourth.continuation(),
        DialogContinuation::Complete
    ));
    assert_eq!(
        (
            pin_calls.load(Ordering::SeqCst),
            normal_calls.load(Ordering::SeqCst)
        ),
        (1, 2)
    );
    client.close().await.unwrap();
}

fn settings() -> tl::enums::PeerNotifySettings {
    let mut bytes = tl::types::PeerNotifySettings::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    tl::enums::PeerNotifySettings::from_bytes(&bytes).unwrap()
}
fn folder_entry() -> tl::enums::Dialog {
    tl::enums::Dialog::Folder(tl::types::DialogFolder {
        pinned: true,
        folder: tl::enums::Folder::Folder(tl::types::Folder {
            autofill_new_broadcasts: false,
            autofill_public_groups: false,
            autofill_new_correspondents: false,
            id: 1,
            title: "private archive title".into(),
            photo: None,
        }),
        peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 77 }),
        top_message: 5,
        unread_muted_peers_count: 1,
        unread_unmuted_peers_count: 1,
        unread_muted_messages_count: 1,
        unread_unmuted_messages_count: 2,
    })
}

#[tokio::test]
async fn folder_and_community_containers_are_explicit_and_never_become_send_targets() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        assert_eq!(
            bytes[..4],
            tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
        );
        let mut person = user(77, 1, 123);
        if let tl::enums::User::User(person) = &mut person {
            person.first_name = Some("private person name".into());
        }
        let mut response = tl::enums::messages::PeerDialogs::from_bytes(&pinned(
            vec![
                folder_entry(),
                dialog(77, 5, true, 0),
                tl::enums::Dialog::Community(tl::types::DialogCommunity {
                    pinned: true,
                    community_id: 88,
                    notify_settings: settings(),
                }),
            ],
            vec![dated(5, 77, 100)],
            vec![person],
        ))
        .unwrap();
        let tl::enums::messages::PeerDialogs::Dialogs(value) = &mut response;
        value.chats.push(tl::enums::Chat::CommunityForbidden(
            tl::types::CommunityForbidden {
                id: 88,
                access_hash: Some(987654321),
                title: "private community title".into(),
            },
        ));
        Ok(response.to_bytes())
    });
    let page = client
        .account(expected())
        .dialogs(DialogFolder::Main, 100, None)
        .await
        .unwrap();
    let archive = &page.dialogs()[0];
    let person = &page.dialogs()[1];
    let community = &page.dialogs()[2];
    assert_eq!(archive.id(), DialogId::Folder(DialogFolder::Archive));
    assert_eq!(archive.name(), Some("private archive title"));
    assert_eq!(archive.unread_count(), Some(3));
    assert!(archive.peer().is_none() && community.peer().is_none());
    assert_eq!(
        community.id(),
        DialogId::Community(PeerId::channel(88).unwrap())
    );
    assert_eq!(community.name(), Some("private community title"));
    assert_eq!(person.name(), Some("private person name"));
    assert!(person.peer().is_some());
    assert!(std::ptr::eq(
        archive.last_message().unwrap(),
        person.last_message().unwrap()
    ));
    for secret in ["private", "987654321"] {
        assert!(!format!("{page:?} {archive:?} {community:?}").contains(secret));
    }
    client.close().await.unwrap();
}

#[tokio::test]
async fn cleared_or_hashless_slices_keep_entries_with_an_explicit_incomplete_tail() {
    use crate::DialogPageIssue;
    let cases = [
        (
            0,
            vec![],
            user(77, 1, 123),
            false,
            Some(DialogPageIssue::MissingAnchor),
        ),
        (
            5,
            vec![tl::enums::Message::Empty(tl::types::MessageEmpty {
                id: 5,
                peer_id: None,
            })],
            user(77, 1, 123),
            false,
            Some(DialogPageIssue::MissingAnchor),
        ),
        (
            5,
            vec![dated(5, 77, 100)],
            user(77, 1 | (1 << 20), 123),
            false,
            Some(DialogPageIssue::MissingPeer),
        ),
        (0, vec![], user(77, 1, 123), true, None),
    ];
    for (top, messages, person, complete, issue) in cases {
        let (_directory, stored) = session().await;
        let response = ordinary(
            vec![dialog(77, top, false, 0)],
            messages,
            vec![person],
            complete,
        );
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let response = response.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4]
                    == tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(pinned(vec![], vec![], vec![]));
                }
                Ok(response)
            }
        });
        let page = client
            .account(expected())
            .dialogs(DialogFolder::Main, 10, None)
            .await
            .unwrap();
        assert_eq!(page.dialogs().len(), 1);
        assert_eq!(page.dialogs()[0].top_message_id(), (top > 0).then_some(top));
        match issue {
            Some(issue) => assert!(
                matches!(page.continuation(), DialogContinuation::Incomplete(found) if *found == issue)
            ),
            None => assert!(matches!(page.continuation(), DialogContinuation::Complete)),
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn cursor_refuses_other_client_folder_account_and_epoch_before_any_rpc() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).unwrap_or_else(|| {
            pinned(
                vec![dialog(77, 0, true, 0), dialog(78, 0, true, 0)],
                vec![],
                vec![],
            )
        }))
    });
    let account = client.account(expected());
    let page = account.dialogs(DialogFolder::Main, 1, None).await.unwrap();
    let DialogContinuation::More(cursor) = page.continuation() else {
        panic!()
    };
    let (_directory2, stored2) = session().await;
    let other = fixture(stored2, ClientOptions::default(), |_, _| async {
        panic!("invalid cursor must stay local")
    });
    assert_eq!(
        other
            .account(expected())
            .dialogs(DialogFolder::Main, 1, Some(cursor))
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidCursor
    );
    assert_eq!(
        account
            .dialogs(DialogFolder::Archive, 1, Some(cursor))
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidCursor
    );
    let wrong = AccountIdentity {
        id: AccountId::new(43).unwrap(),
        ..expected()
    };
    assert_eq!(
        client
            .account(wrong)
            .dialogs(DialogFolder::Main, 1, Some(cursor))
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidCursor
    );
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    assert_eq!(
        account
            .dialogs(DialogFolder::Main, 1, Some(cursor))
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    for limit in [0, 101] {
        assert_eq!(
            other
                .account(expected())
                .dialogs(DialogFolder::Main, limit, None)
                .await
                .unwrap_err()
                .kind(),
            ClientErrorKind::InvalidInput
        );
    }
    let bot = AccountIdentity {
        kind: AccountKind::Bot,
        ..expected()
    };
    assert_eq!(
        other
            .account(bot)
            .dialogs(DialogFolder::Main, 1, None)
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::UnsupportedOperation
    );
    other.close().await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn even_cached_pinned_pages_reverify_identity_and_post_guard_epoch() {
    for change_identity in [false, true] {
        let (_directory, stored) = session().await;
        let checks = Arc::new(AtomicUsize::new(0));
        let calls = checks.clone();
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let calls = calls.clone();
            async move {
                if self_reply(&bytes).is_some() {
                    let id = if calls.fetch_add(1, Ordering::SeqCst) > 0 && change_identity {
                        43
                    } else {
                        42
                    };
                    return Ok(vec![user(id, 1 << 10, 0)].to_bytes());
                }
                Ok(pinned(
                    vec![dialog(77, 0, true, 0), dialog(78, 0, true, 0)],
                    vec![],
                    vec![],
                ))
            }
        });
        let account = client.account(expected());
        let page = account.dialogs(DialogFolder::Main, 1, None).await.unwrap();
        let DialogContinuation::More(cursor) = page.continuation() else {
            panic!()
        };
        let cursor = cursor.clone();
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.inner.slots.available_permits() != 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let gate = client.inner.authorization.clone().write_owned().await;
        let job =
            tokio::spawn(
                async move { account.dialogs(DialogFolder::Main, 1, Some(&cursor)).await },
            );
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.inner.slots.available_permits() == 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if !change_identity {
            client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
        }
        drop(gate);
        let failure = job.await.unwrap().unwrap_err();
        assert_eq!(
            failure.kind(),
            if change_identity {
                ClientErrorKind::IdentityMismatch
            } else {
                ClientErrorKind::InvalidCursor
            }
        );
        assert_eq!(checks.load(Ordering::SeqCst), 2);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn archive_folder_is_an_explicit_request_scope() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        if bytes[..4] == tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes() {
            let request =
                tl::functions::messages::GetPinnedDialogs::from_bytes(&bytes[4..]).unwrap();
            assert_eq!(request.folder_id, 1);
            return Ok(pinned(vec![], vec![], vec![]));
        }
        let request = tl::functions::messages::GetDialogs::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(request.folder_id, Some(1));
        Ok(ordinary(
            vec![dialog(77, 5, false, 1)],
            vec![dated(5, 77, 100)],
            vec![user(77, 1, 123)],
            true,
        ))
    });
    let page = client
        .account(expected())
        .dialogs(DialogFolder::Archive, 10, None)
        .await
        .unwrap();
    assert_eq!(page.dialogs().len(), 1);
    assert!(matches!(page.continuation(), DialogContinuation::Complete));
    client.close().await.unwrap();
}

#[tokio::test]
async fn malformed_dialog_snapshots_and_ordinary_pages_fail_without_guessed_data() {
    let mut negative = dialog(77, 5, true, 0);
    if let tl::enums::Dialog::Dialog(raw) = &mut negative {
        raw.unread_count = -1;
    }
    let mut bad_state =
        tl::enums::messages::PeerDialogs::from_bytes(&pinned(vec![], vec![], vec![])).unwrap();
    let tl::enums::messages::PeerDialogs::Dialogs(raw) = &mut bad_state;
    let tl::enums::updates::State::State(state) = &mut raw.state;
    state.pts = -1;
    let mut long_name = user(77, 1, 123);
    if let tl::enums::User::User(raw) = &mut long_name {
        raw.first_name = Some("x".repeat(4097));
    }
    let mut bad_count =
        tl::enums::messages::Dialogs::from_bytes(&ordinary(vec![], vec![], vec![], false)).unwrap();
    let tl::enums::messages::Dialogs::Slice(raw) = &mut bad_count else {
        unreachable!()
    };
    raw.count = -1;
    let cases = vec![
        (
            true,
            pinned(
                vec![dialog(77, 5, true, 0), dialog(77, 5, true, 0)],
                vec![dated(5, 77, 100)],
                vec![user(77, 1, 123)],
            ),
        ),
        (
            true,
            pinned(
                vec![dialog(77, 5, true, 0)],
                vec![dated(6, 77, 100)],
                vec![user(77, 1, 123)],
            ),
        ),
        (
            true,
            pinned(
                vec![dialog(77, 5, true, 0)],
                vec![dated(5, 78, 100)],
                vec![user(77, 1, 123)],
            ),
        ),
        (
            true,
            pinned(
                vec![dialog(77, 5, true, 0), dialog(78, 5, true, 0)],
                vec![tl::enums::Message::Empty(tl::types::MessageEmpty {
                    id: 5,
                    peer_id: None,
                })],
                vec![],
            ),
        ),
        (
            true,
            pinned(
                vec![negative],
                vec![dated(5, 77, 100)],
                vec![user(77, 1, 123)],
            ),
        ),
        (
            true,
            pinned(
                vec![dialog(77, 5, true, 1)],
                vec![dated(5, 77, 100)],
                vec![],
            ),
        ),
        (true, pinned(vec![dialog(77, -1, true, 0)], vec![], vec![])),
        (
            true,
            pinned(
                (1..=1025).map(|id| dialog(1000 + id, 0, true, 0)).collect(),
                vec![],
                vec![],
            ),
        ),
        (
            true,
            pinned(vec![dialog(77, 0, true, 0)], vec![], vec![long_name]),
        ),
        (true, bad_state.to_bytes()),
        (false, bad_count.to_bytes()),
        (
            false,
            tl::enums::messages::Dialogs::NotModified(tl::types::messages::DialogsNotModified {
                count: 0,
            })
            .to_bytes(),
        ),
        (
            false,
            ordinary(
                vec![dialog(77, 5, false, 0), dialog(78, 6, false, 0)],
                vec![dated(5, 77, 100), dated(6, 78, 200)],
                vec![user(77, 1, 123), user(78, 1, 456)],
                false,
            ),
        ),
        (
            false,
            ordinary(
                vec![
                    dialog(77, 5, false, 0),
                    dialog(78, 6, false, 0),
                    dialog(79, 7, false, 0),
                ],
                vec![dated(5, 77, 100), dated(6, 78, 90), dated(7, 79, 80)],
                vec![],
                true,
            ),
        ),
    ];
    for (pinned_phase, response) in cases {
        let (_directory, stored) = session().await;
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let response = response.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if !pinned_phase
                    && bytes[..4]
                        == tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(pinned(vec![], vec![], vec![]));
                }
                Ok(response)
            }
        });
        let failure = client
            .account(expected())
            .dialogs(DialogFolder::Main, 2, None)
            .await
            .unwrap_err();
        assert_eq!(
            (failure.kind(), failure.outcome()),
            (ClientErrorKind::Decode, Outcome::Unknown)
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn ordinary_equal_date_id_ties_progress_by_dialog_id_and_repeats_fail() {
    let (_directory, stored) = session().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
            {
                return Ok(pinned(vec![], vec![], vec![]));
            }
            let request = tl::functions::messages::GetDialogs::from_bytes(&bytes[4..]).unwrap();
            let id = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert_eq!(request.offset_id, 0);
                    78
                }
                1 => {
                    assert!(
                        matches!(request.offset_peer, tl::enums::InputPeer::User(raw) if raw.user_id == 78)
                    );
                    77
                }
                _ => {
                    assert!(
                        matches!(request.offset_peer, tl::enums::InputPeer::User(raw) if raw.user_id == 77)
                    );
                    77
                }
            };
            Ok(ordinary(
                vec![dialog(id, 5, false, 0)],
                vec![dated(5, id, 100)],
                vec![user(id, 1, 123)],
                false,
            ))
        }
    });
    let account = client.account(expected());
    let first = account.dialogs(DialogFolder::Main, 1, None).await.unwrap();
    let DialogContinuation::More(cursor) = first.continuation() else {
        panic!()
    };
    let second = account
        .dialogs(DialogFolder::Main, 1, Some(cursor))
        .await
        .unwrap();
    let DialogContinuation::More(cursor) = second.continuation() else {
        panic!()
    };
    let failure = account
        .dialogs(DialogFolder::Main, 1, Some(cursor))
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Decode);
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}

#[tokio::test]
async fn pinned_state_never_advances_provisional_pts_and_retained_cursor_keeps_no_client_alive() {
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
    backend
        .set_update_state(UpdateState::Channel { id: 77, pts: 10 })
        .await
        .unwrap();
    let before = backend.updates_state().await.unwrap();
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        Ok(self_reply(&bytes).unwrap_or_else(|| {
            let mut channel = dialog(77, 0, true, 0);
            if let tl::enums::Dialog::Dialog(raw) = &mut channel {
                raw.peer = tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 77 });
                raw.pts = Some(999);
            }
            let mut reply = tl::enums::messages::PeerDialogs::from_bytes(&pinned(
                vec![channel, dialog(78, 0, true, 0)],
                vec![],
                vec![],
            ))
            .unwrap();
            let tl::enums::messages::PeerDialogs::Dialogs(raw) = &mut reply;
            raw.chats.push(tl::enums::Chat::ChannelForbidden(
                tl::types::ChannelForbidden {
                    broadcast: true,
                    megagroup: false,
                    monoforum: false,
                    id: 77,
                    access_hash: 987654321,
                    title: "private channel".into(),
                    until_date: None,
                },
            ));
            reply.to_bytes()
        }))
    });
    let page = client
        .account(expected())
        .dialogs(DialogFolder::Main, 1, None)
        .await
        .unwrap();
    assert_eq!(backend.updates_state().await.unwrap(), before);
    let owner = Arc::downgrade(&client.inner);
    let mut exited = client.inner.shared.exited.subscribe();
    drop(client);
    assert!(owner.upgrade().is_none());
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
    assert_eq!(page.dialogs().len(), 1);
    assert!(matches!(page.continuation(), DialogContinuation::More(_)));
}

#[tokio::test]
async fn timed_out_dialog_rpc_is_owned_joined_and_never_replayed() {
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
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::GetPinnedDialogs::CONSTRUCTOR_ID.to_le_bytes()
                );
                calls.fetch_add(1, Ordering::SeqCst);
                let _stop = Stop(status);
                std::future::pending().await
            }
        },
    );
    let failure = client
        .account(expected())
        .dialogs(DialogFolder::Main, 1, None)
        .await
        .unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::Timeout, Outcome::Unknown)
    );
    client.close().await.unwrap();
    assert!(stopped.load(Ordering::Acquire));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}
