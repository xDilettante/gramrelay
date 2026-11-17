use super::*;
use crate::session::updates::{BatchKind, GapReason, GapScope, UpdateDraft};

fn identity() -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: AccountKind::User,
    }
}
async fn verified(session: &Session) {
    session
        .remember_verified(
            identity(),
            PeerInfo::User {
                id: 42,
                auth: None,
                bot: Some(false),
                is_self: Some(true),
            },
        )
        .await
        .unwrap();
}
fn checkpoint(pts: i32) -> UpdatesState {
    UpdatesState {
        pts,
        qts: 0,
        seq: pts,
        date: 100 + pts,
        channels: vec![ChannelState { id: 9, pts }],
    }
}
fn draft(kind: BatchKind, base: Option<UpdatesState>, pts: i32) -> UpdateDraft {
    UpdateDraft {
        account: identity(),
        kind,
        base,
        checkpoint: checkpoint(pts),
        payload: vec![].into(),
    }
}

#[tokio::test]
async fn resync_may_retire_only_its_exact_inaccessible_channel_after_explicit_ack() {
    for access_lost in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let session = open(
            &directory.path().join("updates.sqlite"),
            SessionOptions::default(),
        )
        .await;
        verified(&session).await;
        let baseline = session
            .stage_updates(draft(BatchKind::Baseline, None, 10))
            .await
            .unwrap();
        session
            .acknowledge_updates(identity(), baseline.id)
            .await
            .unwrap();
        let gap = session
            .record_update_gap(
                identity(),
                GapScope::Channel(9),
                if access_lost {
                    GapReason::ChannelAccessLost
                } else {
                    GapReason::ChannelTooLong
                },
            )
            .await
            .unwrap();
        let mut decision = draft(BatchKind::Resync { gap: gap.id }, Some(checkpoint(10)), 10);
        decision.checkpoint.channels.clear();
        let result = session.stage_updates(decision).await;
        if !access_lost {
            assert!(result.is_err());
        } else {
            let decision = result.unwrap();
            assert_eq!(
                session
                    .updates_view(identity())
                    .unwrap()
                    .acknowledged
                    .unwrap()
                    .channels
                    .len(),
                1
            );
            session
                .acknowledge_updates(identity(), decision.id)
                .await
                .unwrap();
            let view = session.updates_view(identity()).unwrap();
            assert!(view.gap.is_none() && view.acknowledged.unwrap().channels.is_empty());
        }
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn prepared_batch_replays_after_restart_and_only_exact_ack_advances_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let session = open(&path, SessionOptions::default()).await;
    verified(&session).await;
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    assert!(
        session
            .updates_view(identity())
            .unwrap()
            .acknowledged
            .is_none()
    );
    session.close().await.unwrap();
    let session = open(&path, SessionOptions::default()).await;
    let replay = session.updates_view(identity()).unwrap().pending.unwrap();
    assert!(replay == baseline);
    session
        .acknowledge_updates(identity(), replay.id)
        .await
        .unwrap();
    // Lost confirmation is idempotent and does not consume a later batch.
    let mut next = draft(BatchKind::Events, Some(checkpoint(10)), 11);
    next.payload = b"private-event-and-peer-hash".to_vec().into();
    let next = session.stage_updates(next).await.unwrap();
    session
        .acknowledge_updates(identity(), baseline.id)
        .await
        .unwrap();
    let view = session.updates_view(identity()).unwrap();
    assert!(view.acknowledged == Some(checkpoint(10)));
    assert!(view.pending == Some(next.clone()));
    session.close().await.unwrap();
    let session = open(&path, SessionOptions::default()).await;
    assert!(session.updates_view(identity()).unwrap().pending == Some(next.clone()));
    session
        .acknowledge_updates(identity(), next.id)
        .await
        .unwrap();
    assert!(session.updates_view(identity()).unwrap().acknowledged == Some(checkpoint(11)));
    session.close().await.unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if let Ok(bytes) = std::fs::read(file) {
            assert!(
                !bytes
                    .windows(27)
                    .any(|part| part == b"private-event-and-peer-hash")
            );
        }
    }
}

#[tokio::test]
async fn gap_preserves_earlier_events_and_new_loss_invalidates_only_resync_decision() {
    let directory = tempfile::tempdir().unwrap();
    let session = open(
        &directory.path().join("updates.sqlite"),
        SessionOptions::default(),
    )
    .await;
    verified(&session).await;
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    session
        .acknowledge_updates(identity(), baseline.id)
        .await
        .unwrap();
    let events = session
        .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
        .await
        .unwrap();
    let gap = session
        .record_update_gap(identity(), GapScope::Channel(9), GapReason::ChannelTooLong)
        .await
        .unwrap();
    assert!(session.updates_view(identity()).unwrap().pending == Some(events.clone()));
    session
        .acknowledge_updates(identity(), events.id)
        .await
        .unwrap();
    assert!(session.updates_view(identity()).unwrap().gap == Some(gap));
    assert!(
        session
            .stage_updates(draft(BatchKind::Events, Some(checkpoint(11)), 12))
            .await
            .is_err()
    );
    let decision = session
        .stage_updates(draft(
            BatchKind::Resync { gap: gap.id },
            Some(checkpoint(11)),
            20,
        ))
        .await
        .unwrap();
    let newer = session
        .record_update_gap(identity(), GapScope::Account, GapReason::QueueOverflow)
        .await
        .unwrap();
    assert!(newer.id != gap.id);
    assert!(
        session
            .acknowledge_updates(identity(), decision.id)
            .await
            .is_err()
    );
    assert!(session.updates_view(identity()).unwrap().acknowledged == Some(checkpoint(11)));
    let decision = session
        .stage_updates(draft(
            BatchKind::Resync { gap: newer.id },
            Some(checkpoint(11)),
            21,
        ))
        .await
        .unwrap();
    session
        .acknowledge_updates(identity(), decision.id)
        .await
        .unwrap();
    let view = session.updates_view(identity()).unwrap();
    assert!(view.gap.is_none() && view.pending.is_none());
    assert!(view.acknowledged == Some(checkpoint(21)));
    session.close().await.unwrap();
}

#[tokio::test]
async fn baseline_lost_before_ack_becomes_gap_without_losing_any_events() {
    let directory = tempfile::tempdir().unwrap();
    let session = open(
        &directory.path().join("updates.sqlite"),
        SessionOptions::default(),
    )
    .await;
    verified(&session).await;
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    let gap = session
        .record_update_gap(identity(), GapScope::Account, GapReason::QueueOverflow)
        .await
        .unwrap();
    let view = session.updates_view(identity()).unwrap();
    assert!(view.pending.is_none() && view.acknowledged.is_none() && view.gap == Some(gap));
    assert!(
        session
            .acknowledge_updates(identity(), baseline.id)
            .await
            .is_err()
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn owned_capacity_bounds_do_not_reject_valid_redecoded_batch_vectors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let options = SessionOptions::default().max_channels(3);
    let session = open(&path, options.clone()).await;
    verified(&session).await;
    let mut baseline = draft(BatchKind::Baseline, None, 10);
    baseline.checkpoint.channels = vec![
        ChannelState { id: 9, pts: 10 },
        ChannelState { id: 10, pts: 10 },
        ChannelState { id: 11, pts: 10 },
    ];
    assert_eq!(baseline.checkpoint.channels.capacity(), 3);
    let baseline = session.stage_updates(baseline).await.unwrap();
    session.close().await.unwrap();
    let session = open(&path, options).await;
    assert!(session.updates_view(identity()).unwrap().pending == Some(baseline));
    session.close().await.unwrap();
}

#[tokio::test]
async fn staging_requires_exact_account_base_and_monotone_complete_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let session = open(
        &directory.path().join("updates.sqlite"),
        SessionOptions::default(),
    )
    .await;
    verified(&session).await;
    assert!(
        session
            .stage_updates(draft(BatchKind::Events, None, 10))
            .await
            .is_err()
    );
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    session
        .acknowledge_updates(identity(), baseline.id)
        .await
        .unwrap();
    for case in 0..10 {
        let mut next = draft(BatchKind::Events, Some(checkpoint(10)), 11);
        match case {
            0 => next.account.id = AccountId::new(43).unwrap(),
            1 => next.base = Some(checkpoint(9)),
            2 => next.checkpoint.pts = 9,
            3 => next.checkpoint.qts = -1,
            4 => next.checkpoint.date = 1,
            5 => next.checkpoint.seq = 9,
            6 => next.checkpoint.channels.clear(),
            7 => next.checkpoint.channels[0].pts = 9,
            8 => next
                .checkpoint
                .channels
                .push(next.checkpoint.channels[0].clone()),
            _ => next.kind = BatchKind::Baseline,
        }
        assert!(session.stage_updates(next).await.is_err(), "case {case}");
        assert!(session.updates_view(identity()).unwrap().pending.is_none());
    }
    let mut oversized = draft(BatchKind::Events, Some(checkpoint(10)), 11);
    oversized.payload = vec![1; crate::session::updates::MAX_BATCH_BYTES + 1].into();
    assert!(
        matches!(session.stage_updates(oversized).await, Err(error) if error.kind() == SessionErrorKind::Capacity)
    );
    let events = session
        .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
        .await
        .unwrap();
    let foreign = AccountIdentity {
        id: AccountId::new(43).unwrap(),
        ..identity()
    };
    assert!(
        session
            .acknowledge_updates(foreign, events.id)
            .await
            .is_err()
    );
    assert!(session.updates_view(foreign).is_err());
    let mut forged = serde_json::to_value(events.id).unwrap();
    forged["nonce"][0] = ((forged["nonce"][0].as_u64().unwrap() + 1) % 256).into();
    let forged = serde_json::from_value(forged).unwrap();
    assert!(
        session
            .acknowledge_updates(identity(), forged)
            .await
            .is_err()
    );
    assert!(session.updates_view(identity()).unwrap().acknowledged == Some(checkpoint(10)));
    session
        .acknowledge_updates(identity(), events.id)
        .await
        .unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn competing_stages_and_duplicate_acks_are_serialized_without_checkpoint_loss() {
    let directory = tempfile::tempdir().unwrap();
    let session = open(
        &directory.path().join("updates.sqlite"),
        SessionOptions::default(),
    )
    .await;
    verified(&session).await;
    let (first, second) = tokio::join!(
        session.stage_updates(draft(BatchKind::Baseline, None, 10)),
        session.stage_updates(draft(BatchKind::Baseline, None, 20))
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let pending = first.or(second).unwrap();
    let (first, second) = tokio::join!(
        session.acknowledge_updates(identity(), pending.id),
        session.acknowledge_updates(identity(), pending.id)
    );
    first.unwrap();
    second.unwrap();
    let view = session.updates_view(identity()).unwrap();
    assert!(view.pending.is_none() && view.acknowledged == Some(pending.draft.checkpoint.clone()));
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_stage_keeps_its_owned_write_and_returns_as_replay() {
    let directory = tempfile::tempdir().unwrap();
    let session = open(
        &directory.path().join("updates.sqlite"),
        SessionOptions::default(),
    )
    .await;
    verified(&session).await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let owner = session.clone();
    let blocker = tokio::spawn(async move {
        owner
            .inner
            .database
            .call(move |_| {
                let _ = entered.send(());
                released.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(())
            })
            .await
    });
    waiting.await.unwrap();
    let owner = session.clone();
    let write = tokio::spawn(async move {
        owner
            .stage_updates(draft(BatchKind::Baseline, None, 10))
            .await
    });
    // Admission is visible in the worker's bounded queue before cancelling.
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if session.inner.database.available_slots() == 14 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    write.abort();
    let _ = write.await;
    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    session.inner.database.call(|_| Ok(())).await.unwrap();
    let view = session.updates_view(identity()).unwrap();
    assert!(view.pending.is_some() && view.acknowledged.is_none());
    session.close().await.unwrap();
}

#[tokio::test]
async fn late_stage_or_ack_commit_never_reports_success_and_reopen_reads_durable_truth() {
    for ack in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.sqlite");
        let session = open(&path, SessionOptions::default()).await;
        verified(&session).await;
        let baseline = session
            .stage_updates(draft(BatchKind::Baseline, None, 10))
            .await
            .unwrap();
        session
            .acknowledge_updates(identity(), baseline.id)
            .await
            .unwrap();
        let pending = if ack {
            Some(
                session
                    .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        session
            .inner
            .database
            .call(|native| {
                native.expire_on_commit();
                Ok(())
            })
            .await
            .unwrap();
        let error = if let Some(pending) = pending {
            session
                .acknowledge_updates(identity(), pending.id)
                .await
                .unwrap_err()
        } else {
            session
                .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
                .await
                .err()
                .unwrap()
        };
        assert_eq!(error.kind(), SessionErrorKind::Fenced);
        assert!(session.updates_view(identity()).is_err());
        let _ = session.close().await;
        let session = open(&path, SessionOptions::default()).await;
        let view = session.updates_view(identity()).unwrap();
        assert!(view.acknowledged == Some(checkpoint(if ack { 11 } else { 10 })));
        assert_eq!(view.pending.is_some(), !ack);
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn quarantined_journal_survives_until_explicit_discard_without_reusing_ids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let session = open(&path, SessionOptions::default()).await;
    verified(&session).await;
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    session
        .acknowledge_updates(identity(), baseline.id)
        .await
        .unwrap();
    let pending = session
        .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
        .await
        .unwrap();
    session.quarantine_authorization().await.unwrap();
    assert!(
        matches!(session.updates_view(identity()), Err(error) if error.kind()==SessionErrorKind::Quarantined)
    );
    session.close().await.unwrap();
    let session = open(&path, SessionOptions::default()).await;
    assert!(session.is_quarantined().unwrap());
    assert!(
        session
            .inner
            .database
            .shared
            .read(|view| view.snapshot.updates.pending == Some(pending.clone()))
            .unwrap()
    );
    session.forget_quarantined_authorization().await.unwrap();
    verified(&session).await;
    let next = session
        .stage_updates(draft(BatchKind::Baseline, None, 20))
        .await
        .unwrap();
    let old = serde_json::to_value(pending.id).unwrap();
    let new = serde_json::to_value(next.id).unwrap();
    assert!(new["serial"].as_u64().unwrap() > old["serial"].as_u64().unwrap());
    assert!(
        session
            .acknowledge_updates(identity(), pending.id)
            .await
            .is_err()
    );
    assert!(
        session
            .updates_view(identity())
            .unwrap()
            .acknowledged
            .is_none()
    );
    session.close().await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess fixture: synthetic temporary journal only"]
async fn update_journal_crash_child() {
    let path = std::path::PathBuf::from(std::env::var_os("TG_UPDATE_CRASH_SESSION").unwrap());
    let marker = std::env::var_os("TG_UPDATE_CRASH_MARKER").unwrap();
    let boundary: u8 = std::env::var("TG_UPDATE_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let session = open(
        &path,
        SessionOptions::default().lease(Duration::from_millis(500)),
    )
    .await;
    verified(&session).await;
    let baseline = session
        .stage_updates(draft(BatchKind::Baseline, None, 10))
        .await
        .unwrap();
    session
        .acknowledge_updates(identity(), baseline.id)
        .await
        .unwrap();
    if boundary > 0 {
        let pending = session
            .stage_updates(draft(BatchKind::Events, Some(checkpoint(10)), 11))
            .await
            .unwrap();
        if boundary > 1 {
            session
                .acknowledge_updates(identity(), pending.id)
                .await
                .unwrap();
        }
    }
    std::fs::write(marker, b"ready").unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn killed_process_before_append_after_append_and_after_ack_recovers_only_committed_state() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for boundary in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.sqlite");
        let marker = directory.path().join("ready");
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "session::tests::updates::update_journal_crash_child",
                ])
                .env("TG_UPDATE_CRASH_SESSION", &path)
                .env("TG_UPDATE_CRASH_MARKER", &marker)
                .env("TG_UPDATE_CRASH_BOUNDARY", boundary.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "crash fixture exited early"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let session = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match Session::open(
                    &path,
                    SessionBinding::new("demo", 1).unwrap(),
                    SessionKey::from_bytes([7; 32]),
                    SessionOptions::default(),
                )
                .await
                {
                    Ok(session) => break session,
                    Err(error) if error.kind() == SessionErrorKind::Owned => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(error) => panic!("reopen failed: {error}"),
                }
            }
        })
        .await
        .unwrap();
        let view = session.updates_view(identity()).unwrap();
        assert!(view.acknowledged == Some(checkpoint(if boundary == 2 { 11 } else { 10 })));
        assert_eq!(view.pending.is_some(), boundary == 1);
        session.close().await.unwrap();
    }
}
