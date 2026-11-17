use super::*;
use crate::{UpdateBatchKind, UpdateItem, UpdateMode};

fn state(pts: i32) -> tl::enums::updates::State {
    tl::enums::updates::State::State(tl::types::updates::State {
        pts,
        qts: 0,
        seq: pts,
        date: pts.saturating_add(100),
        unread_count: 0,
    })
}
fn empty_difference() -> Vec<u8> {
    tl::enums::updates::Difference::Empty(tl::types::updates::DifferenceEmpty {
        date: 110,
        seq: 10,
    })
    .to_bytes()
}
async fn seed(stored: &Session, channel: Option<bool>) {
    use grammers_session::types::{ChannelState, PeerAuth, PeerInfo, UpdatesState};
    stored
        .remember_verified(
            expected(),
            PeerInfo::User {
                id: 42,
                auth: None,
                bot: Some(false),
                is_self: Some(true),
            },
        )
        .await
        .unwrap();
    if let Some(hash) = channel {
        stored
            .cache_peers(vec![PeerInfo::Channel {
                id: 9,
                auth: hash.then(|| PeerAuth::from_hash(123)),
                kind: None,
            }])
            .await
            .unwrap();
    }
    let checkpoint = UpdatesState {
        pts: 10,
        qts: 0,
        date: 110,
        seq: 10,
        channels: channel
            .map(|_| vec![ChannelState { id: 9, pts: 5 }])
            .unwrap_or_default(),
    };
    let pending = stored
        .stage_updates(crate::session::updates::UpdateDraft {
            account: expected(),
            kind: crate::session::updates::BatchKind::Baseline,
            base: None,
            checkpoint,
            payload: vec![].into(),
        })
        .await
        .unwrap();
    stored
        .acknowledge_updates(expected(), pending.id)
        .await
        .unwrap();
}
fn injecting<F, Fut>(
    session: Session,
    options: ClientOptions,
    callback: F,
) -> (
    Client,
    Arc<Mutex<Option<grammers_mtsender::SenderPoolHandle>>>,
)
where
    F: Fn(i32, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Vec<u8>, grammers_mtsender::InvocationError>>
        + Send
        + 'static,
{
    let handle = Arc::new(Mutex::new(None));
    let slot = handle.clone();
    let client = Client::with_factory(
        session,
        options,
        Box::new(move |session, id| {
            let pool = SenderPool::with_test_rpc(session, id, callback);
            *slot.lock().unwrap() = Some(pool.handle.thin.clone());
            pool
        }),
    )
    .unwrap();
    (client, handle)
}
fn raw_update(update: tl::enums::Update) -> grammers_session::updates::UpdatesLike {
    grammers_session::updates::UpdatesLike::Updates(
        tl::types::UpdateShort { update, date: 110 }.into(),
    )
}
fn channel_too_long() -> Vec<u8> {
    let mut dialog = tl::types::Dialog::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    dialog.extend_from_slice(&0u32.to_le_bytes());
    dialog.extend_from_slice(
        &tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 9 }).to_bytes(),
    );
    for _ in 0..7 {
        dialog.extend_from_slice(&0i32.to_le_bytes());
    }
    dialog.extend_from_slice(&tl::types::PeerNotifySettings::CONSTRUCTOR_ID.to_le_bytes());
    dialog.extend_from_slice(&0u32.to_le_bytes());
    tl::enums::updates::ChannelDifference::TooLong(tl::types::updates::ChannelDifferenceTooLong {
        r#final: true,
        timeout: None,
        dialog: tl::enums::Dialog::from_bytes(&dialog).unwrap(),
        messages: vec![],
        chats: vec![],
        users: vec![],
    })
    .to_bytes()
}
fn full_channel(pts: i32, excessive: bool) -> Vec<u8> {
    let mut full = tl::types::ChannelFull::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    full.extend_from_slice(&0u32.to_le_bytes());
    full.extend_from_slice(&0u32.to_le_bytes());
    full.extend_from_slice(&9i64.to_le_bytes());
    full.extend_from_slice(&"private channel metadata".to_owned().to_bytes());
    for _ in 0..3 {
        full.extend_from_slice(&0i32.to_le_bytes());
    }
    full.extend_from_slice(&tl::enums::Photo::Empty(tl::types::PhotoEmpty { id: 0 }).to_bytes());
    full.extend_from_slice(&tl::types::PeerNotifySettings::CONSTRUCTOR_ID.to_le_bytes());
    full.extend_from_slice(&0u32.to_le_bytes());
    full.extend_from_slice(&Vec::<tl::enums::BotInfo>::new().to_bytes());
    full.extend_from_slice(&pts.to_le_bytes());
    tl::enums::messages::ChatFull::Full(tl::types::messages::ChatFull {
        full_chat: tl::enums::ChatFull::from_bytes(&full).unwrap(),
        chats: vec![],
        users: if excessive {
            (1000..2025).map(|id| user(id, 0, 0)).collect()
        } else {
            vec![]
        },
    })
    .to_bytes()
}
#[tokio::test]
async fn start_now_and_whole_difference_stage_before_ack_and_replay_without_new_reads() {
    let (_directory, stored) = session().await;
    let inspection = stored.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(state(10).to_bytes());
            }
            assert_eq!(
                bytes[..4],
                tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes()
            );
            let request = tl::functions::updates::GetDifference::from_bytes(&bytes[4..]).unwrap();
            assert_eq!(request.pts, 10);
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(
                tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
                    new_messages: vec![
                        message(1, 42, "private first", false),
                        message(2, 42, "private second", false),
                    ],
                    new_encrypted_messages: vec![],
                    other_updates: vec![],
                    chats: vec![],
                    users: vec![],
                    state: state(12),
                })
                .to_bytes(),
            )
        }
    });
    let account = client.account(expected());
    let mut receiver = account.updates(UpdateMode::StartNow).unwrap();
    let Some(UpdateItem::Batch(baseline)) = receiver.next_batch(Duration::ZERO).await.unwrap()
    else {
        panic!("baseline required")
    };
    assert_eq!(baseline.kind(), UpdateBatchKind::Baseline);
    assert!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .is_none()
    );
    receiver.acknowledge(&baseline).await.unwrap();
    let Some(UpdateItem::Batch(batch)) = receiver.next_batch(Duration::ZERO).await.unwrap() else {
        panic!("difference required")
    };
    assert_eq!(batch.updates().len(), 2);
    assert_eq!(batch.kind(), UpdateBatchKind::Events);
    assert!(!format!("{batch:?}").contains("private"));
    let Some(UpdateItem::Batch(replay)) = receiver.next_batch(Duration::ZERO).await.unwrap() else {
        panic!("replay required")
    };
    assert_eq!(replay.id(), batch.id());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .unwrap()
            .pts,
        10
    );
    receiver.acknowledge(&replay).await.unwrap();
    assert_eq!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .unwrap()
            .pts,
        12
    );
    assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
    client.close().await.unwrap();
}

#[tokio::test]
async fn malformed_initial_state_and_duplicate_peer_entities_persist_gap_without_ack() {
    for case in 0..3 {
        let (_directory, stored) = session().await;
        let observed = stored.clone();
        let client = fixture(
            stored,
            ClientOptions::default(),
            move |_, bytes| async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes() {
                    return Ok(state(match case {
                        0 => -1,
                        1 => i32::MAX,
                        _ => 10,
                    })
                    .to_bytes());
                }
                Ok(
                    tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
                        new_messages: vec![],
                        new_encrypted_messages: vec![],
                        other_updates: vec![],
                        chats: vec![],
                        users: vec![user(43, 0, 0), user(43, 0, 0)],
                        state: state(12),
                    })
                    .to_bytes(),
                )
            },
        );
        let mut receiver = client
            .account(expected())
            .updates(UpdateMode::StartNow)
            .unwrap();
        if case == 2 {
            let UpdateItem::Batch(baseline) =
                receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
            else {
                panic!()
            };
            receiver.acknowledge(&baseline).await.unwrap();
        }
        let UpdateItem::Gap(gap) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
        else {
            panic!("invalid input became a batch")
        };
        assert_eq!(gap.reason(), UpdateGapReason::Malformed);
        assert!(observed.updates_view(expected()).unwrap().gap.is_some());
        assert_eq!(
            observed
                .updates_view(expected())
                .unwrap()
                .acknowledged
                .map(|state| state.pts),
            if case == 2 { Some(10) } else { None }
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn resume_without_baseline_requires_explicit_resync_and_ack_before_starting() {
    let (_directory, stored) = session().await;
    let inspection = stored.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            assert_eq!(
                bytes[..4],
                tl::functions::updates::GetState::CONSTRUCTOR_ID.to_le_bytes()
            );
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(state(20).to_bytes())
        }
    });
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let UpdateItem::Gap(required) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(required.reason(), UpdateGapReason::MissingBaseline);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let decision = receiver.resync_from_current(&required).await.unwrap();
    assert_eq!(decision.kind(), UpdateBatchKind::Resync);
    assert!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .is_none()
    );
    receiver.acknowledge(&decision).await.unwrap();
    let view = inspection.updates_view(expected()).unwrap();
    assert!(view.gap.is_none());
    assert_eq!(view.acknowledged.unwrap().pts, 20);
    assert_eq!(
        receiver
            .resync_from_current(&required)
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidUpdate
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn account_too_long_is_durable_and_never_advances_old_checkpoint() {
    let (_directory, stored) = session().await;
    seed(&stored, None).await;
    let inspection = stored.clone();
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        Ok(
            tl::enums::updates::Difference::TooLong(tl::types::updates::DifferenceTooLong {
                pts: 999,
            })
            .to_bytes(),
        )
    });
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let UpdateItem::Gap(required) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(required.reason(), UpdateGapReason::AccountTooLong);
    assert_eq!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .unwrap()
            .pts,
        10
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn missing_channel_hash_and_access_loss_are_explicit_and_retirement_is_scoped() {
    for hash in [false, true] {
        let (_directory, stored) = session().await;
        seed(&stored, Some(hash)).await;
        let inspection = stored.clone();
        let channel_calls = Arc::new(AtomicUsize::new(0));
        let seen = channel_calls.clone();
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let seen = seen.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(empty_difference());
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::updates::GetChannelDifference::CONSTRUCTOR_ID.to_le_bytes()
                );
                seen.fetch_add(1, Ordering::SeqCst);
                let request =
                    tl::functions::updates::GetChannelDifference::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.pts, 5);
                Err(grammers_mtsender::InvocationError::Rpc(
                    grammers_mtsender::RpcError {
                        code: 406,
                        name: "CHANNEL_PRIVATE".into(),
                        value: None,
                        caused_by: None,
                    },
                ))
            }
        });
        let mut receiver = client
            .account(expected())
            .updates(UpdateMode::Resume)
            .unwrap();
        // Empty account difference changes nothing and may end an idle poll.
        let first = receiver.next_batch(Duration::ZERO).await.unwrap();
        let item = if let Some(item) = first {
            item
        } else {
            receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
        };
        let UpdateItem::Gap(required) = item else {
            panic!()
        };
        assert_eq!(
            required.reason(),
            if hash {
                UpdateGapReason::ChannelAccessLost
            } else {
                UpdateGapReason::MissingHash
            }
        );
        assert_eq!(channel_calls.load(Ordering::SeqCst), usize::from(hash));
        if hash {
            let decision = receiver.retire_channel(&required).await.unwrap();
            assert_eq!(
                inspection
                    .updates_view(expected())
                    .unwrap()
                    .acknowledged
                    .unwrap()
                    .channels
                    .len(),
                1
            );
            receiver.acknowledge(&decision).await.unwrap();
            assert!(
                inspection
                    .updates_view(expected())
                    .unwrap()
                    .acknowledged
                    .unwrap()
                    .channels
                    .is_empty()
            );
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn raw_malformed_and_overflow_are_durable_and_overflow_requires_new_generation() {
    for overflow in [false, true] {
        let (_directory, stored) = session().await;
        seed(&stored, None).await;
        let inspection = stored.clone();
        let (client, handle) = injecting(stored, ClientOptions::default(), |_, bytes| async move {
            Ok(self_reply(&bytes).unwrap_or_else(empty_difference))
        });
        let mut receiver = client
            .account(expected())
            .updates(UpdateMode::Resume)
            .unwrap();
        assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
        let handle = handle.lock().unwrap().clone().unwrap();
        if overflow {
            let mut lost = false;
            for _ in 0..1000 {
                if !handle
                    .test_push_update(grammers_session::updates::UpdatesLike::ConnectionClosed)
                {
                    lost = true;
                    break;
                }
            }
            assert!(lost);
        } else {
            assert!(
                handle.test_push_update(grammers_session::updates::UpdatesLike::MalformedUpdates)
            );
        }
        let UpdateItem::Gap(required) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(
            required.reason(),
            if overflow {
                UpdateGapReason::QueueOverflow
            } else {
                UpdateGapReason::Malformed
            }
        );
        assert_eq!(required.restart_required(), overflow);
        if overflow {
            assert_eq!(
                receiver
                    .resync_from_current(&required)
                    .await
                    .unwrap_err()
                    .kind(),
                ClientErrorKind::RestartRequired
            );
        }
        assert_eq!(
            inspection
                .updates_view(expected())
                .unwrap()
                .acknowledged
                .unwrap()
                .pts,
            10
        );
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn owned_receiver_claim_and_pending_batch_survive_cancellation_and_encrypted_restart() {
    let (directory, stored) = session().await;
    seed(&stored, None).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let arrived = entered.clone();
    let released = release.clone();
    let client = fixture(
        stored,
        ClientOptions::default().max_pending(1),
        move |_, bytes| {
            let arrived = arrived.clone();
            let released = released.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes()
                );
                arrived.notify_one();
                released.notified().await;
                Ok(
                    tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
                        new_messages: vec![message(1, 42, "private pending", false)],
                        new_encrypted_messages: vec![],
                        other_updates: vec![],
                        chats: vec![],
                        users: vec![],
                        state: state(11),
                    })
                    .to_bytes(),
                )
            }
        },
    );
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let owner = tokio::spawn(async move { receiver.next_batch(Duration::ZERO).await });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    owner.abort();
    let _ = owner.await;
    assert_eq!(
        client
            .account(expected())
            .updates(UpdateMode::Resume)
            .unwrap_err()
            .kind(),
        ClientErrorKind::UpdateReceiverOwned
    );
    assert!(matches!(client.reserve(),Err(failure) if failure.kind()==ClientErrorKind::Capacity));
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.inner.slots.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let UpdateItem::Batch(before) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
    else {
        panic!()
    };
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
        Ok(self_reply(&bytes).expect("pending replay cannot fetch new updates"))
    });
    let mut restored = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let UpdateItem::Batch(after) = restored.next_batch(Duration::ZERO).await.unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(before.id(), after.id());
    assert_eq!(after.updates().len(), 1);
    assert_eq!(
        restored.acknowledge(&before).await.unwrap_err().kind(),
        ClientErrorKind::InvalidUpdate
    );
    restored.acknowledge(&after).await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn receiver_timeout_joins_pending_rpc_and_leaves_ack_unchanged() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (_directory, stored) = session().await;
    seed(&stored, None).await;
    let inspection = stored.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let witness = dropped.clone();
    let client = fixture(
        stored,
        ClientOptions::default().request_timeout(Duration::from_millis(150)),
        move |_, bytes| {
            let witness = witness.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                let _guard = Dropped(witness);
                std::future::pending().await
            }
        },
    );
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let error = receiver
        .next_batch(Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::Timeout);
    assert_eq!(error.outcome(), Outcome::Unknown);
    client.close().await.unwrap();
    assert!(dropped.load(Ordering::Acquire));
    assert!(inspection.remembered_account().is_err());
}

#[tokio::test]
async fn sliced_differences_and_empty_checkpoint_changes_require_whole_batch_ack() {
    for empty in [false, true] {
        let (_directory, stored) = session().await;
        seed(&stored, None).await;
        let inspection = stored.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client =
            fixture(stored, ClientOptions::default(), move |_, bytes| {
                let seen = seen.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    let request =
                        tl::functions::updates::GetDifference::from_bytes(&bytes[4..]).unwrap();
                    let index = seen.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(request.pts, if index == 0 { 10 } else { 11 });
                    if empty {
                        return Ok(tl::enums::updates::Difference::Empty(
                            tl::types::updates::DifferenceEmpty { date: 111, seq: 11 },
                        )
                        .to_bytes());
                    }
                    if index == 0 {
                        Ok(tl::enums::updates::Difference::Slice(
                            tl::types::updates::DifferenceSlice {
                                new_messages: vec![message(1, 42, "first slice", false)],
                                new_encrypted_messages: vec![],
                                other_updates: vec![],
                                chats: vec![],
                                users: vec![],
                                intermediate_state: state(11),
                            },
                        )
                        .to_bytes())
                    } else {
                        Ok(tl::enums::updates::Difference::Difference(
                            tl::types::updates::Difference {
                                new_messages: vec![message(2, 42, "last slice", false)],
                                new_encrypted_messages: vec![],
                                other_updates: vec![],
                                chats: vec![],
                                users: vec![],
                                state: state(12),
                            },
                        )
                        .to_bytes())
                    }
                }
            });
        let mut receiver = client
            .account(expected())
            .updates(UpdateMode::Resume)
            .unwrap();
        let UpdateItem::Batch(first) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(first.updates().len(), usize::from(!empty));
        assert_eq!(
            inspection
                .updates_view(expected())
                .unwrap()
                .acknowledged
                .unwrap()
                .seq,
            10
        );
        receiver.acknowledge(&first).await.unwrap();
        if !empty {
            let UpdateItem::Batch(second) =
                receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
            else {
                panic!()
            };
            assert_ne!(first.id(), second.id());
            receiver.acknowledge(&second).await.unwrap();
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn socket_channel_hint_keeps_old_pts_and_does_not_ack_the_server_hint() {
    let (_directory, stored) = session().await;
    seed(&stored, Some(true)).await;
    let inspection = stored.clone();
    let calls = Arc::new(Mutex::new(vec![]));
    let seen = calls.clone();
    let (client, handle) = injecting(stored, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(empty_difference());
            }
            let request =
                tl::functions::updates::GetChannelDifference::from_bytes(&bytes[4..]).unwrap();
            seen.lock().unwrap().push(request.pts);
            Ok(tl::enums::updates::ChannelDifference::Empty(
                tl::types::updates::ChannelDifferenceEmpty {
                    r#final: true,
                    pts: 5,
                    timeout: None,
                },
            )
            .to_bytes())
        }
    });
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
    assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
    assert!(
        handle
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .test_push_update(raw_update(
                tl::types::UpdateChannelTooLong {
                    channel_id: 9,
                    pts: Some(999)
                }
                .into()
            ))
    );
    assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
    assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
    assert_eq!(*calls.lock().unwrap(), vec![5, 5]);
    assert_eq!(
        inspection
            .updates_view(expected())
            .unwrap()
            .acknowledged
            .unwrap()
            .channels[0]
            .pts,
        5
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn channel_too_long_resyncs_only_its_channel_and_bounds_recovery_entities() {
    for case in 0..3 {
        let excessive = case == 1;
        let access_lost = case == 2;
        let (_directory, stored) = session().await;
        seed(&stored, Some(true)).await;
        let inspection = stored.clone();
        let client = fixture(
            stored,
            ClientOptions::default(),
            move |_, bytes| async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(empty_difference());
                }
                if bytes[..4]
                    == tl::functions::updates::GetChannelDifference::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(channel_too_long());
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::channels::GetFullChannel::CONSTRUCTOR_ID.to_le_bytes()
                );
                if access_lost {
                    return Err(grammers_mtsender::InvocationError::Rpc(
                        grammers_mtsender::RpcError {
                            code: 406,
                            name: "CHANNEL_PRIVATE".into(),
                            value: None,
                            caused_by: None,
                        },
                    ));
                }
                Ok(full_channel(8, excessive))
            },
        );
        let mut receiver = client
            .account(expected())
            .updates(UpdateMode::Resume)
            .unwrap();
        assert!(receiver.next_batch(Duration::ZERO).await.unwrap().is_none());
        let UpdateItem::Gap(required) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(required.reason(), UpdateGapReason::ChannelTooLong);
        let decision = receiver.resync_from_current(&required).await;
        if access_lost {
            assert_eq!(decision.unwrap_err().outcome(), Outcome::Rejected);
            let UpdateItem::Gap(required) =
                receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
            else {
                panic!()
            };
            assert_eq!(required.reason(), UpdateGapReason::ChannelAccessLost);
            let decision = receiver.retire_channel(&required).await.unwrap();
            receiver.acknowledge(&decision).await.unwrap();
            assert!(
                inspection
                    .updates_view(expected())
                    .unwrap()
                    .acknowledged
                    .unwrap()
                    .channels
                    .is_empty()
            );
        } else if excessive {
            assert!(decision.is_err());
        } else {
            let decision = decision.unwrap();
            assert_eq!(
                inspection
                    .updates_view(expected())
                    .unwrap()
                    .acknowledged
                    .unwrap()
                    .channels[0]
                    .pts,
                5
            );
            receiver.acknowledge(&decision).await.unwrap();
            let view = inspection.updates_view(expected()).unwrap();
            assert_eq!(view.acknowledged.as_ref().unwrap().pts, 10);
            assert_eq!(view.acknowledged.unwrap().channels[0].pts, 8);
            assert!(view.gap.is_none());
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn checkpoint_capacity_failure_never_publishes_a_batch_or_advances_durable_ack() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let stored = Session::open(
        &path,
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default().max_snapshot_bytes(2048),
    )
    .await
    .unwrap();
    seed(&stored, None).await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        Ok(
            tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
                new_messages: vec![message(1, 42, &"x".repeat(8192), false)],
                new_encrypted_messages: vec![],
                other_updates: vec![],
                chats: vec![],
                users: vec![],
                state: state(11),
            })
            .to_bytes(),
        )
    });
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    assert_eq!(
        receiver
            .next_batch(Duration::ZERO)
            .await
            .unwrap_err()
            .outcome(),
        Outcome::Unknown
    );
    client.close().await.unwrap();
    let restored = Session::open(
        &path,
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    let view = restored.updates_view(expected()).unwrap();
    assert!(view.pending.is_none());
    assert_eq!(view.acknowledged.unwrap().pts, 10);
    restored.close().await.unwrap();
}

#[tokio::test]
async fn unknown_private_channel_keeps_response_routing_peers_before_exposing_its_gap() {
    let (_directory, stored) = session().await;
    seed(&stored, None).await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        if bytes[..4] == tl::functions::updates::GetDifference::CONSTRUCTOR_ID.to_le_bytes() {
            let channel = tl::enums::Chat::from_bytes(&{
                let mut bytes = tl::types::Channel::CONSTRUCTOR_ID.to_le_bytes().to_vec();
                bytes.extend_from_slice(&(1u32 << 13).to_le_bytes());
                bytes.extend_from_slice(&0u32.to_le_bytes());
                bytes.extend_from_slice(&9i64.to_le_bytes());
                bytes.extend_from_slice(&123i64.to_le_bytes());
                bytes.extend_from_slice(&"private channel".to_owned().to_bytes());
                bytes.extend_from_slice(&tl::enums::ChatPhoto::Empty.to_bytes());
                bytes.extend_from_slice(&100i32.to_le_bytes());
                bytes
            })
            .unwrap();
            return Ok(tl::enums::updates::Difference::Difference(
                tl::types::updates::Difference {
                    new_messages: vec![],
                    new_encrypted_messages: vec![],
                    other_updates: vec![
                        tl::types::UpdateChannelTooLong {
                            channel_id: 9,
                            pts: Some(7),
                        }
                        .into(),
                    ],
                    chats: vec![channel],
                    users: vec![],
                    state: state(11),
                },
            )
            .to_bytes());
        }
        assert_eq!(
            bytes[..4],
            tl::functions::channels::GetFullChannel::CONSTRUCTOR_ID.to_le_bytes()
        );
        let request = tl::functions::channels::GetFullChannel::from_bytes(&bytes[4..]).unwrap();
        assert!(
            matches!(request.channel,tl::enums::InputChannel::Channel(channel) if channel.channel_id==9 && channel.access_hash==123)
        );
        Ok(full_channel(8, false))
    });
    let mut receiver = client
        .account(expected())
        .updates(UpdateMode::Resume)
        .unwrap();
    let UpdateItem::Gap(required) = receiver.next_batch(Duration::ZERO).await.unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(required.reason(), UpdateGapReason::MissingBaseline);
    let decision = receiver.resync_from_current(&required).await.unwrap();
    receiver.acknowledge(&decision).await.unwrap();
    client.close().await.unwrap();
}
