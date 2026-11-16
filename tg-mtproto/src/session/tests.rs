use super::*;
use grammers_session::{
    Session as _,
    types::{ChannelState, PeerAuth, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use std::time::Duration;

async fn open(path: &Path, options: SessionOptions) -> Session {
    Session::open(
        path,
        SessionBinding::new("demo", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        options,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn valid_incremental_state_reaches_the_configured_channel_count() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(
        &dir.path().join("session.sqlite"),
        SessionOptions::default().max_channels(1),
    )
    .await;
    let result = session
        .inner
        .set_update_state(UpdateState::Channel { id: 1, pts: 1 })
        .await;
    session.close().await.unwrap();
    result.expect("one channel is within a one-channel semantic limit");
}
#[tokio::test]
async fn oversized_all_state_is_rejected_before_entering_a_blocked_worker() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(
        &dir.path().join("session.sqlite"),
        SessionOptions::default().max_channels(1).max_pending(2),
    )
    .await;
    let worker = session.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let active = tokio::spawn(async move {
        worker
            .inner
            .database
            .call(move |_| {
                let _ = entered.send(());
                let _ = blocked.recv();
                Ok(())
            })
            .await
    });
    waiting.await.unwrap();
    let state = UpdatesState {
        channels: Vec::with_capacity(1024),
        ..Default::default()
    };
    let result = tokio::time::timeout(
        Duration::from_millis(150),
        session.inner.set_update_state(UpdateState::All(state)),
    )
    .await;
    release.send(()).unwrap();
    active.await.unwrap().unwrap();
    session.close().await.unwrap();
    assert!(
        matches!(result,Ok(Err(error)) if error.kind()==SessionErrorKind::Capacity),
        "oversized retained input must be rejected before queued ownership"
    );
}
#[tokio::test]
async fn a_commit_finishing_after_the_owner_deadline_never_reports_success() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(
        &dir.path().join("session.sqlite"),
        SessionOptions::default(),
    )
    .await;
    let result = session
        .inner
        .database
        .call(|native| {
            native.expire_on_commit();
            native.mutate(|snapshot| {
                snapshot.home_dc = if snapshot.home_dc == 1 { 2 } else { 1 };
                Ok(())
            })
        })
        .await;
    let _ = session.close().await;
    assert_eq!(result.unwrap_err().kind(), SessionErrorKind::Fenced);
}
#[tokio::test]
async fn an_open_commit_finishing_after_deadline_releases_ownership_without_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let mut options = SessionOptions::default().lease(Duration::from_millis(100));
    options.after_open_commit = Some(Duration::from_millis(150));
    let result = Session::open(
        &path,
        SessionBinding::new("demo", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        options,
    )
    .await;
    let fenced = matches!(&result,Err(error) if error.kind()==SessionErrorKind::Fenced);
    if let Ok(session) = result {
        let _ = session.close().await;
    }
    let recovered = open(&path, SessionOptions::default()).await;
    recovered.close().await.unwrap();
    assert!(
        fenced,
        "expired ownership must not be published as a successfully opened session"
    );
}

fn self_peer() -> PeerInfo {
    PeerInfo::User {
        id: 42,
        auth: Some(PeerAuth::from_hash(123456789012345)),
        bot: Some(false),
        is_self: Some(true),
    }
}

#[tokio::test]
async fn keys_peers_identity_and_only_acknowledged_state_survive_encrypted_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let session = open(&path, SessionOptions::default()).await;
    let mut dc = session.inner.dc_option(1).unwrap().unwrap();
    dc.auth_key = Some([0xa5; 256]);
    session.inner.set_dc_option(&dc).await.unwrap();
    session.inner.cache_peer(self_peer()).await.unwrap();
    session
        .inner
        .database
        .call(|native| {
            native.mutate(|snapshot| {
                snapshot.account = Some(AccountIdentity {
                    id: AccountId::new(42).unwrap(),
                    kind: AccountKind::User,
                });
                snapshot.acknowledged = Some(UpdatesState {
                    pts: 10,
                    qts: 3,
                    date: 100,
                    seq: 7,
                    channels: vec![ChannelState { id: 1, pts: 20 }],
                });
                Ok(())
            })
        })
        .await
        .unwrap();
    session
        .inner
        .set_update_state(UpdateState::All(UpdatesState {
            pts: 999,
            qts: 999,
            date: 999,
            seq: 999,
            channels: vec![],
        }))
        .await
        .unwrap();
    // An unrelated persistent mutation must not serialize provisional pts=999.
    session
        .inner
        .cache_peer(PeerInfo::Chat { id: 123 })
        .await
        .unwrap();
    assert_eq!(session.inner.updates_state().await.unwrap().pts, 999);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (nonce, ciphertext, revision): (Vec<u8>, Vec<u8>, i64) = connection
        .query_row(
            "SELECT nonce,ciphertext,revision FROM mtproto_session",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(nonce.len(), 24);
    assert!(revision >= 4);
    assert!(!ciphertext.windows(32).any(|bytes| bytes == [7; 32]));
    for suffix in ["", "-wal", "-shm"] {
        let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if let Ok(bytes) = std::fs::read(file) {
            for marker in [
                vec![0xa5; 64],
                b"a5".repeat(32),
                vec![7; 32],
                b"123456789012345".to_vec(),
            ] {
                assert!(!bytes.windows(marker.len()).any(|window| window == marker));
            }
        }
    }
    drop(connection);
    session.close().await.unwrap();
    let restored = open(&path, SessionOptions::default()).await;
    assert_eq!(restored.remembered_account().unwrap().unwrap().id.get(), 42);
    assert_eq!(
        restored.inner.dc_option(1).unwrap().unwrap().auth_key,
        Some([0xa5; 256])
    );
    assert_eq!(
        restored.inner.peer(PeerId::self_user()).await.unwrap(),
        Some(self_peer())
    );
    let state = restored.inner.updates_state().await.unwrap();
    assert_eq!(state.pts, 10);
    assert_eq!(state.channels[0].pts, 20);
    restored.close().await.unwrap();
}

#[tokio::test]
async fn admitted_cancelled_work_keeps_its_slot_and_publishes_disk_and_cache_together() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let session = open(&path, SessionOptions::default().max_pending(1)).await;
    let worker = session.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let (committed, done) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        worker
            .inner
            .database
            .call(move |native| {
                let _ = entered.send(());
                let _ = blocked.recv();
                let result = native.mutate(|snapshot| {
                    snapshot.peers.push(self_peer());
                    snapshot.account = Some(AccountIdentity {
                        id: AccountId::new(42).unwrap(),
                        kind: AccountKind::User,
                    });
                    Ok(())
                });
                let _ = committed.send(());
                result
            })
            .await
    });
    waiting.await.unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(
        session
            .inner
            .cache_peer(PeerInfo::Chat { id: 1 })
            .await
            .unwrap_err()
            .kind(),
        SessionErrorKind::Capacity
    );
    assert!(session.remembered_account().unwrap().is_none());
    release.send(()).unwrap();
    done.await.unwrap();
    assert_eq!(session.remembered_account().unwrap().unwrap().id.get(), 42);
    session.close().await.unwrap();
    let restored = open(&path, SessionOptions::default()).await;
    assert_eq!(restored.remembered_account().unwrap().unwrap().id.get(), 42);
    restored.close().await.unwrap();
}

#[tokio::test]
async fn close_cancelled_while_draining_still_releases_the_session_on_every_clone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let session = open(&path, SessionOptions::default()).await;
    let worker = session.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let pending = tokio::spawn(async move {
        worker
            .inner
            .database
            .call(move |_| {
                let _ = entered.send(());
                let _ = blocked.recv();
                Ok(())
            })
            .await
    });
    waiting.await.unwrap();
    let closer = session.clone();
    let mut health = session.inner.database.shared.health.subscribe();
    let close = tokio::spawn(async move { closer.close().await });
    while *health.borrow_and_update() != Health::Closing {
        health.changed().await.unwrap();
    }
    close.abort();
    let _ = close.await;
    assert_eq!(
        session.remembered_account().unwrap_err().kind(),
        SessionErrorKind::Closed
    );
    release.send(()).unwrap();
    pending.await.unwrap().unwrap();
    session.close().await.unwrap();
    let restored = open(&path, SessionOptions::default()).await;
    restored.close().await.unwrap();
}

#[tokio::test]
async fn idle_heartbeat_renews_ownership_beyond_the_original_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let options = SessionOptions::default().lease(Duration::from_millis(300));
    let session = open(&path, options.clone()).await;
    let connection = rusqlite::Connection::open(&path).unwrap();
    let first: i64 = connection
        .query_row("SELECT lease_until_ms FROM mtproto_session", [], |row| {
            row.get(0)
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let current: i64 = connection
                .query_row("SELECT lease_until_ms FROM mtproto_session", [], |row| {
                    row.get(0)
                })
                .unwrap();
            if current > first + 300 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(session.remembered_account().is_ok());
    let second = Session::open(
        &path,
        SessionBinding::new("demo", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        options,
    )
    .await
    .unwrap_err();
    assert_eq!(second.kind(), SessionErrorKind::Owned);
    drop(connection);
    session.close().await.unwrap();
}

#[tokio::test]
async fn an_expired_owner_cannot_overwrite_or_release_a_successor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let session = open(
        &path,
        SessionOptions::default().lease(Duration::from_millis(150)),
    )
    .await;
    let connection = rusqlite::Connection::open(&path).unwrap();
    let until: i64 = connection
        .query_row("SELECT lease_until_ms FROM mtproto_session", [], |row| {
            row.get(0)
        })
        .unwrap();
    let worker = session.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let old = tokio::spawn(async move {
        worker
            .inner
            .database
            .call(move |native| {
                let _ = entered.send(());
                let _ = blocked.recv();
                native.mutate(|snapshot| {
                    snapshot.home_dc = if snapshot.home_dc == 1 { 2 } else { 1 };
                    Ok(())
                })
            })
            .await
    });
    waiting.await.unwrap();
    tokio::time::sleep(Duration::from_millis(
        (until - now().unwrap()).max(0) as u64 + 15,
    ))
    .await;
    let successor = open(&path, SessionOptions::default()).await;
    let owner: Vec<u8> = connection
        .query_row("SELECT owner FROM mtproto_session", [], |row| row.get(0))
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(
        old.await.unwrap().unwrap_err().kind(),
        SessionErrorKind::Fenced
    );
    assert_eq!(
        session.close().await.unwrap_err().kind(),
        SessionErrorKind::Fenced
    );
    let current: Vec<u8> = connection
        .query_row("SELECT owner FROM mtproto_session", [], |row| row.get(0))
        .unwrap();
    assert_eq!(owner, current);
    assert!(successor.remembered_account().is_ok());
    drop(connection);
    successor.close().await.unwrap();
}

#[tokio::test]
async fn unchanged_metadata_does_not_reencrypt_or_advance_the_snapshot_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.sqlite");
    let session = open(&path, SessionOptions::default()).await;
    session.inner.cache_peer(self_peer()).await.unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let read = || {
        connection
            .query_row(
                "SELECT revision,nonce,ciphertext FROM mtproto_session",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .unwrap()
    };
    let first = read();
    session.inner.cache_peer(self_peer()).await.unwrap();
    assert_eq!(read(), first);
    session
        .inner
        .cache_peer(PeerInfo::Chat { id: 1 })
        .await
        .unwrap();
    let changed = read();
    assert!(changed.0 > first.0);
    assert_ne!(changed.1, first.1);
    assert_ne!(changed.2, first.2);
    drop(connection);
    session.close().await.unwrap();
}
