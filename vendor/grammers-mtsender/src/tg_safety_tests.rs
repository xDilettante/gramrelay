// Local downstream regressions. Synthetic state and loopback sockets only.
use super::*;
use grammers_session::{SessionData, storages::MemorySession};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "tg-test-kit")]
use tl::Deserializable;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::time::timeout;

#[cfg(feature = "tg-test-kit")]
#[tokio::test]
async fn fixture_uses_real_tl_bytes_and_owned_concurrent_children_without_network() {
    use tl::{Identifiable, Serializable};
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let pool =
        SenderPool::with_test_rpc(Arc::new(MemorySession::default()), 1, move |dc, bytes| {
            let seen = seen.clone();
            let barrier = barrier.clone();
            async move {
                assert_eq!(dc, 2);
                assert_eq!(
                    &bytes[..4],
                    &tl::functions::Ping::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request = tl::functions::Ping::from_bytes(&bytes[4..]).unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                barrier.wait().await;
                Ok(tl::enums::Pong::Pong(tl::types::Pong {
                    msg_id: 1,
                    ping_id: request.ping_id,
                })
                .to_bytes())
            }
        });
    let runner = tokio::spawn(pool.runner.run());
    let first = pool
        .handle
        .invoke_in_dc(2, &tl::functions::Ping { ping_id: 11 });
    let second = pool
        .handle
        .invoke_in_dc(2, &tl::functions::Ping { ping_id: 22 });
    let (first, second) = timeout(Duration::from_secs(1), async {
        tokio::join!(first, second)
    })
    .await
    .unwrap();
    assert!(matches!(first.unwrap(),tl::enums::Pong::Pong(pong) if pong.ping_id==11));
    assert!(matches!(second.unwrap(),tl::enums::Pong::Pong(pong) if pong.ping_id==22));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    pool.handle.quit();
    runner.await.unwrap();
}

#[cfg(feature = "tg-test-kit")]
#[tokio::test]
async fn fixture_update_loss_and_eof_match_the_real_runner() {
    let mut pool = SenderPool::with_test_rpc(Arc::new(MemorySession::default()), 1, |_, _| async {
        Err(InvocationError::InvalidDc)
    });
    for _ in 0..pool.updates.max_capacity() {
        assert!(pool.handle.test_push_update(UpdatesLike::ConnectionClosed));
    }
    assert!(!pool.handle.test_push_update(UpdatesLike::ConnectionClosed));
    assert!(*pool.handle.update_loss().borrow());
    pool.handle.quit();
    pool.runner.run().await;
    while pool.updates.recv().await.is_some() {}
    assert!(!pool.handle.test_push_update(UpdatesLike::ConnectionClosed));
}

#[cfg(feature = "tg-test-kit")]
#[tokio::test]
async fn fixture_quit_joins_pending_callbacks_and_releases_waiters() {
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = started.clone();
    let finished = dropped.clone();
    let pool = SenderPool::with_test_rpc(Arc::new(MemorySession::default()), 1, move |_, _| {
        let observed = observed.clone();
        let finished = finished.clone();
        async move {
            let _guard = Dropped(finished);
            observed.notify_one();
            std::future::pending().await
        }
    });
    let controller = pool.handle.thin.clone();
    let runner = tokio::spawn(pool.runner.run());
    let call = tokio::spawn(async move {
        pool.handle
            .invoke_in_dc(2, &tl::functions::Ping { ping_id: 1 })
            .await
    });
    timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    controller.quit();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert!(
        timeout(Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[cfg(feature = "tg-test-kit")]
#[tokio::test]
async fn fixture_preserves_no_retries_for_confirmed_rpc_refusals() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let pool = SenderPool::with_test_rpc(Arc::new(MemorySession::default()), 1, move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        async {
            Err(InvocationError::Rpc(crate::RpcError {
                code: 420,
                name: "FLOOD_WAIT".into(),
                value: Some(17),
                caused_by: None,
            }))
        }
    });
    let runner = tokio::spawn(pool.runner.run());
    assert!(
        matches!(timeout(Duration::from_secs(1),pool.handle.invoke_in_dc(2,&tl::functions::Ping{ping_id:1})).await.unwrap(),Err(InvocationError::Rpc(error)) if error.value==Some(17))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    pool.handle.quit();
    runner.await.unwrap();
}

#[tokio::test]
async fn no_retries_never_reconnects_after_encrypted_loopback_io_loss() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = match listener.local_addr().unwrap() {
        std::net::SocketAddr::V4(address) => address,
        _ => unreachable!(),
    };
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = connections.clone();
    let (stop, mut stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = tokio::select! { biased;
                _=&mut stopped => break,
                socket=listener.accept()=>socket.unwrap(),
            };
            count.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0; 8192];
            assert!(
                timeout(Duration::from_secs(2), stream.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap()
                    > 0
            );
        }
    });
    let mut data = SessionData::default();
    let dc_id = data.home_dc;
    let dc = data.dc_options.get_mut(&dc_id).unwrap();
    dc.ipv4 = address;
    dc.auth_key = Some([7; 256]);
    let config = ConnectionParams {
        retry_policy: Box::new(crate::NoRetries),
        ..ConnectionParams::default()
    };
    let pool = SenderPool::with_configuration(Arc::new(MemorySession::from(data)), 1, config);
    let runner = tokio::spawn(pool.runner.run());
    assert!(
        timeout(
            Duration::from_secs(3),
            pool.handle
                .invoke_in_dc(dc_id, &tl::functions::Ping { ping_id: 42 })
        )
        .await
        .unwrap()
        .is_err()
    );
    pool.handle.quit();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap();
    let _ = stop.send(());
    server.await.unwrap();
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn no_retries_returns_flood_wait_without_sleep_or_replay() {
    let config = ConnectionParams {
        retry_policy: Box::new(crate::NoRetries),
        ..ConnectionParams::default()
    };
    let pool = SenderPool::with_configuration(Arc::new(MemorySession::default()), 1, config);
    let runner = tokio::spawn(pool.runner.run());
    let error = InvocationError::Rpc(crate::RpcError {
        code: 420,
        name: "FLOOD_WAIT".into(),
        value: Some(17),
        caused_by: None,
    });
    assert!(
        matches!(timeout(Duration::from_millis(200), pool.handle.check_retry(error, NonZeroU32::new(1).unwrap(),Duration::ZERO)).await.unwrap(),ControlFlow::Break(InvocationError::Rpc(error)) if error.value==Some(17))
    );
    pool.handle.quit();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn calls_started_after_quit_fail_without_driving_the_runner() {
    let pool = SenderPool::new(Arc::new(MemorySession::default()), 1);
    assert!(pool.handle.quit());
    assert!(!pool.handle.disconnect_from_dc(1));
    assert!(matches!(
        timeout(
            Duration::from_millis(100),
            pool.handle.raw_invoke_in_dc(1, vec![0; 4])
        )
        .await
        .unwrap(),
        Err(InvocationError::Dropped(_))
    ));
}

#[tokio::test]
async fn quitting_interrupts_an_in_progress_connection_setup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut data = SessionData::default();
    let dc = data.dc_options.get_mut(&data.home_dc).unwrap();
    dc.ipv4 = match listener.local_addr().unwrap() {
        std::net::SocketAddr::V4(address) => address,
        _ => unreachable!(),
    };
    dc.auth_key = Some([0x5a; 256]);
    let dc_id = data.home_dc;
    let pool = SenderPool::new(Arc::new(MemorySession::from(data)), 1);
    let handle = pool.handle.thin;
    let runner = tokio::spawn(pool.runner.run());
    let call = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .invoke_in_dc(dc_id, &tl::functions::Ping { ping_id: 1 })
                .await
        }
    });
    let (mut socket, _) = timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        1
    );
    assert!(handle.quit());
    if timeout(Duration::from_millis(300), async {
        while !runner.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err()
    {
        runner.abort();
        let _ = runner.await;
        call.abort();
        let _ = call.await;
        panic!("quit did not interrupt pending connection setup");
    }
    runner.await.unwrap();
    assert!(call.await.unwrap().is_err());
    let mut rest = Vec::new();
    timeout(Duration::from_secs(1), socket.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn quit_waits_until_every_spawned_child_has_been_dropped() {
    let mut pool = SenderPool::new(Arc::new(MemorySession::default()), 1);
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let (started, waiting) = oneshot::channel();
    pool.runner.connection_pool.spawn(async move {
        let _guard = guard;
        let _ = started.send(());
        std::future::pending().await
    });
    waiting.await.unwrap();
    assert!(pool.handle.quit());
    let runner = tokio::spawn(pool.runner.run());
    if timeout(Duration::from_millis(300), async {
        while !runner.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err()
    {
        runner.abort();
        let _ = runner.await;
        panic!("quit did not join the child task tree");
    }
    runner.await.unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn full_update_queue_reports_sticky_loss_without_blocking_rpc_or_shutdown() {
    let config = ConnectionParams {
        updates_channel_capacity: std::num::NonZeroUsize::new(1).unwrap(),
        ..ConnectionParams::default()
    };
    let mut pool = SenderPool::with_configuration(Arc::new(MemorySession::default()), 1, config);
    let loss = pool.handle.update_loss();
    assert!(!*loss.borrow());
    forward_update(
        &pool.runner.updates_tx,
        &pool.runner.update_loss,
        UpdatesLike::ConnectionClosed,
    )
    .unwrap();
    assert!(matches!(
        forward_update(
            &pool.runner.updates_tx,
            &pool.runner.update_loss,
            UpdatesLike::ConnectionClosed
        ),
        Err(ForwardError::Full)
    ));
    assert!(*loss.borrow());
    let runner = tokio::spawn(pool.runner.run());
    assert!(matches!(
        timeout(
            Duration::from_secs(1),
            pool.handle.raw_invoke_in_dc(999, vec![0; 4])
        )
        .await
        .unwrap(),
        Err(InvocationError::InvalidDc)
    ));
    pool.updates.recv().await.unwrap();
    tokio::task::yield_now().await;
    assert!(*loss.borrow());
    pool.handle.quit();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_closed_updates_receiver_also_latches_loss() {
    let pool = SenderPool::new(Arc::new(MemorySession::default()), 1);
    let loss = pool.handle.update_loss();
    drop(pool.updates);
    assert!(matches!(
        forward_update(
            &pool.runner.updates_tx,
            &pool.runner.update_loss,
            UpdatesLike::ConnectionClosed
        ),
        Err(ForwardError::Closed)
    ));
    assert!(*loss.borrow());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_panic_in_connection_setup_joins_existing_children_before_unwinding() {
    let config = ConnectionParams {
        transport: Box::new(|| panic!("synthetic transport factory panic")),
        ..ConnectionParams::default()
    };
    let mut data = SessionData::default();
    let dc_id = data.home_dc;
    data.dc_options.get_mut(&dc_id).unwrap().ipv4 = "127.0.0.1:1".parse().unwrap();
    let mut pool = SenderPool::with_configuration(Arc::new(MemorySession::from(data)), 1, config);
    let dropped = Arc::new(AtomicBool::new(false));
    struct SlowDrop {
        dropped: Arc<AtomicBool>,
        entered: std::sync::mpsc::Sender<()>,
        released: std::sync::mpsc::Receiver<()>,
    }
    impl Drop for SlowDrop {
        fn drop(&mut self) {
            let _ = self.entered.send(());
            let _ = self.released.recv_timeout(Duration::from_secs(2));
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    let (entered, drop_started) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let guard = SlowDrop {
        dropped: dropped.clone(),
        entered,
        released,
    };
    let (started, waiting) = oneshot::channel();
    pool.runner.connection_pool.spawn(async move {
        let _guard = guard;
        let _ = started.send(());
        std::future::pending().await
    });
    waiting.await.unwrap();
    let releaser = std::thread::spawn(move || {
        drop_started.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let _ = release.send(());
    });
    let runner = tokio::spawn(pool.runner.run());
    let _ = pool.handle.raw_invoke_in_dc(dc_id, vec![0; 4]).await;
    assert!(
        timeout(Duration::from_secs(1), runner)
            .await
            .unwrap()
            .unwrap_err()
            .is_panic()
    );
    let joined_child = dropped.load(Ordering::SeqCst);
    releaser.join().unwrap();
    assert!(
        joined_child,
        "child must have completed Drop before runner join returns"
    );
}
