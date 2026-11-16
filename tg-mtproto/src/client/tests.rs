use super::*;
use tl::{Deserializable, Identifiable, Serializable};

async fn session() -> (tempfile::TempDir, Session) {
    let directory = tempfile::tempdir().unwrap();
    let session = Session::open(
        directory.path().join("session.sqlite"),
        crate::SessionBinding::new("demo", 1).unwrap(),
        crate::SessionKey::from_bytes([7; 32]),
        crate::SessionOptions::default(),
    )
    .await
    .unwrap();
    (directory, session)
}
fn fixture<F, Fut>(session: Session, options: ClientOptions, callback: F) -> Client
where
    F: Fn(i32, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Vec<u8>, grammers_mtsender::InvocationError>>
        + Send
        + 'static,
{
    Client::with_factory(
        session,
        options,
        Box::new(move |session, api_id| SenderPool::with_test_rpc(session, api_id, callback)),
    )
    .unwrap()
}
fn pong(bytes: &[u8]) -> Vec<u8> {
    assert_eq!(
        &bytes[..4],
        &tl::functions::Ping::CONSTRUCTOR_ID.to_le_bytes()
    );
    let ping = tl::functions::Ping::from_bytes(&bytes[4..]).unwrap();
    tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 1,
        ping_id: ping.ping_id,
    })
    .to_bytes()
}

#[tokio::test]
async fn typed_raw_rpc_uses_the_pinned_wire_path_and_close_affects_every_clone() {
    let (_directory, session) = session().await;
    let client = fixture(
        session.clone(),
        ClientOptions::default(),
        |_, bytes| async move { Ok(pong(&bytes)) },
    );
    let response = client
        .invoke(&tl::functions::Ping { ping_id: 42 })
        .await
        .unwrap();
    assert!(matches!(response,tl::enums::Pong::Pong(value) if value.ping_id==42));
    let clone = client.clone();
    client.close().await.unwrap();
    assert_eq!(
        clone
            .invoke(&tl::functions::Ping { ping_id: 1 })
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(
        session.remembered_account().unwrap_err().kind(),
        crate::SessionErrorKind::Closed
    );
}

#[tokio::test]
async fn cancelling_a_caller_keeps_the_admitted_slot_until_real_rpc_completion() {
    let (_directory, session) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = entered.clone();
    let gate = release.clone();
    let client = fixture(
        session,
        ClientOptions::default().max_pending(1),
        move |_, bytes| {
            let seen = seen.clone();
            let gate = gate.clone();
            async move {
                seen.notify_one();
                gate.notified().await;
                Ok(pong(&bytes))
            }
        },
    );
    let caller = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 1 }).await }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    caller.abort();
    let _ = caller.await;
    let error = client
        .invoke(&tl::functions::Ping { ping_id: 2 })
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::Capacity);
    assert_eq!(error.outcome(), Outcome::NotSent);
    assert_eq!(client.inner.slots.available_permits(), 0);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.inner.slots.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close().await.unwrap();
}

static BOMB_DROPPED: AtomicBool = AtomicBool::new(false);
struct Bomb;
impl Drop for Bomb {
    fn drop(&mut self) {
        BOMB_DROPPED.store(true, Ordering::Release);
        panic!("synthetic return-value destructor");
    }
}
impl tl::Deserializable for Bomb {
    fn deserialize(buffer: &mut tl::deserialize::Cursor<'_>) -> tl::deserialize::Result<Self> {
        let _ = tl::enums::Pong::deserialize(buffer)?;
        Ok(Self)
    }
}
struct BombRequest;
impl tl::Serializable for BombRequest {
    fn serialize(&self, sink: &mut impl FnMut(&[u8])) {
        tl::functions::Ping { ping_id: 1 }.serialize(sink);
    }
}
impl tl::RemoteCall for BombRequest {
    type Return = Bomb;
}

#[tokio::test]
async fn a_panicking_cancelled_result_cannot_kill_the_supervisor_and_strand_close() {
    BOMB_DROPPED.store(false, Ordering::Release);
    let (_directory, session) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let gate = release.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        let observed = observed.clone();
        let gate = gate.clone();
        async move {
            observed.notify_one();
            gate.notified().await;
            Ok(pong(&bytes))
        }
    });
    let caller = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(&BombRequest).await }
    });
    entered.notified().await;
    caller.abort();
    let _ = caller.await;
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !BOMB_DROPPED.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), client.close()).await,
        Ok(Ok(()))
    ));
}

#[test]
fn runtime_without_a_timer_is_rejected_before_claim_transfer_and_can_still_close() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_directory, session) = session().await;
        let client = fixture(
            session.clone(),
            ClientOptions::default(),
            |_, bytes| async move { Ok(pong(&bytes)) },
        );
        let failure = client
            .invoke(&tl::functions::Ping { ping_id: 1 })
            .await
            .unwrap_err();
        assert_eq!(failure.kind(), ClientErrorKind::Runtime);
        assert_eq!(failure.outcome(), Outcome::NotSent);
        client.close().await.unwrap();
        assert_eq!(
            session.remembered_account().unwrap_err().kind(),
            crate::SessionErrorKind::Closed
        );
    });
}

#[tokio::test]
async fn an_owned_job_expiring_before_raw_dispatch_is_not_sent() {
    let (_directory, session) = session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let client = fixture(
        session,
        ClientOptions::default().request_timeout(Duration::from_millis(20)),
        move |_, bytes| {
            count.fetch_add(1, Ordering::AcqRel);
            async move { Ok(pong(&bytes)) }
        },
    );
    let reservation = client.reserve().unwrap();
    let failure = client
        .submit(reservation, |context| async move {
            std::thread::sleep(Duration::from_millis(40));
            context
                .invoke::<tl::functions::Ping>(tl::functions::Ping { ping_id: 1 }.to_bytes())
                .await
        })
        .await
        .unwrap_err();
    assert_eq!(failure.outcome(), Outcome::NotSent);
    assert_eq!(calls.load(Ordering::Acquire), 0);
    client.close().await.unwrap();
}

#[tokio::test]
async fn an_observed_success_cannot_later_expire_and_close_the_generation() {
    let (_directory, session) = session().await;
    let mut options = ClientOptions::default()
        .max_pending(1)
        .request_timeout(Duration::from_millis(20));
    options.after_reply = Some(Duration::from_millis(80));
    let client = fixture(session, options, |_, bytes| async move { Ok(pong(&bytes)) });
    client
        .invoke(&tl::functions::Ping { ping_id: 1 })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    client
        .invoke(&tl::functions::Ping { ping_id: 2 })
        .await
        .unwrap();
    client.close().await.unwrap();
}

struct Release(Option<std::sync::mpsc::Sender<()>>);
impl Release {
    fn now(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.now();
    }
}
struct SlowDrop {
    started: Arc<tokio::sync::Notify>,
    released: std::sync::mpsc::Receiver<()>,
    finished: Arc<AtomicBool>,
}
impl Drop for SlowDrop {
    fn drop(&mut self) {
        self.started.notify_one();
        let _ = self.released.recv_timeout(Duration::from_secs(3));
        self.finished.store(true, Ordering::Release);
    }
}
struct SlowFixture {
    client: Client,
    entered: Arc<tokio::sync::Notify>,
    dropping: Arc<tokio::sync::Notify>,
    finished: Arc<AtomicBool>,
    release: Release,
}
fn slow_fixture(session: Session, options: ClientOptions, fail_ping_one: bool) -> SlowFixture {
    let entered = Arc::new(tokio::sync::Notify::new());
    let dropping = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let (release, released) = std::sync::mpsc::channel();
    let guard = Arc::new(Mutex::new(Some(SlowDrop {
        started: dropping.clone(),
        released,
        finished: finished.clone(),
    })));
    let notify = entered.clone();
    let client = fixture(session, options, move |_, bytes| {
        let ping = tl::functions::Ping::from_boxed_bytes(&bytes).unwrap();
        let fail = fail_ping_one && ping.ping_id == 1;
        let guard = if fail {
            None
        } else {
            guard.lock().unwrap().take()
        };
        let notify = notify.clone();
        async move {
            if fail {
                return Err(grammers_mtsender::InvocationError::Io(
                    std::io::Error::other("PRIVATE_IO"),
                ));
            }
            let _guard = guard.unwrap();
            notify.notify_one();
            std::future::pending().await
        }
    });
    SlowFixture {
        client,
        entered,
        dropping,
        finished,
        release: Release(Some(release)),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn timeout_keeps_the_slot_and_session_claim_until_the_driver_tree_has_joined() {
    let (_directory, session) = session().await;
    let mut fixture = slow_fixture(
        session.clone(),
        ClientOptions::default()
            .max_pending(1)
            .request_timeout(Duration::from_millis(200)),
        false,
    );
    let call = tokio::spawn({
        let client = fixture.client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 2 }).await }
    });
    tokio::time::timeout(Duration::from_secs(1), fixture.entered.notified())
        .await
        .unwrap();
    let failure = call.await.unwrap().unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Timeout);
    assert_eq!(failure.outcome(), Outcome::Unknown);
    tokio::time::timeout(Duration::from_secs(1), fixture.dropping.notified())
        .await
        .unwrap();
    assert!(!fixture.finished.load(Ordering::Acquire));
    assert_eq!(fixture.client.inner.slots.available_permits(), 0);
    assert_eq!(
        session.close().await.unwrap_err().kind(),
        crate::SessionErrorKind::Owned
    );
    assert!(Client::new(session, ClientOptions::default()).is_err());
    fixture.release.now();
    tokio::time::timeout(Duration::from_secs(1), fixture.client.close())
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.finished.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn cancelling_a_close_wait_never_aborts_its_owned_cleanup() {
    let (_directory, session) = session().await;
    let mut fixture = slow_fixture(
        session.clone(),
        ClientOptions::default().max_pending(1),
        false,
    );
    let call = tokio::spawn({
        let client = fixture.client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 2 }).await }
    });
    tokio::time::timeout(Duration::from_secs(1), fixture.entered.notified())
        .await
        .unwrap();
    let closing = tokio::spawn({
        let client = fixture.client.clone();
        async move { client.close().await }
    });
    tokio::time::timeout(Duration::from_secs(1), fixture.dropping.notified())
        .await
        .unwrap();
    assert!(!closing.is_finished());
    closing.abort();
    let _ = closing.await;
    assert_eq!(call.await.unwrap().unwrap_err().outcome(), Outcome::Unknown);
    assert_eq!(fixture.client.inner.slots.available_permits(), 0);
    fixture.release.now();
    tokio::time::timeout(Duration::from_secs(1), fixture.client.close())
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.finished.load(Ordering::Acquire));
    assert_eq!(
        session.remembered_account().unwrap_err().kind(),
        crate::SessionErrorKind::Closed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_fenced_completion_keeps_every_outstanding_slot_through_driver_join() {
    let (_directory, session) = session().await;
    let mut fixture = slow_fixture(session, ClientOptions::default().max_pending(2), true);
    let call = tokio::spawn({
        let client = fixture.client.clone();
        async move { client.invoke(&tl::functions::Ping { ping_id: 2 }).await }
    });
    tokio::time::timeout(Duration::from_secs(1), fixture.entered.notified())
        .await
        .unwrap();
    let failure = fixture
        .client
        .invoke(&tl::functions::Ping { ping_id: 1 })
        .await
        .unwrap_err();
    assert_eq!(failure.outcome(), Outcome::Unknown);
    assert!(!format!("{failure:?} {failure}").contains("PRIVATE_IO"));
    assert!(std::error::Error::source(&failure).is_none());
    tokio::time::timeout(Duration::from_secs(1), fixture.dropping.notified())
        .await
        .unwrap();
    assert_eq!(fixture.client.inner.slots.available_permits(), 0);
    assert_eq!(call.await.unwrap().unwrap_err().outcome(), Outcome::Unknown);
    fixture.release.now();
    fixture.client.close().await.unwrap();
}

static FENCE_SESSION: Mutex<Option<Session>> = Mutex::new(None);
#[derive(Debug)]
struct FencingResult;
impl Deserializable for FencingResult {
    fn deserialize(cursor: &mut tl::Cursor<'_>) -> tl::deserialize::Result<Self> {
        let _ = tl::enums::Pong::deserialize(cursor)?;
        FENCE_SESSION.lock().unwrap().as_ref().unwrap().test_fence();
        Ok(Self)
    }
}
struct FencingRequest;
impl Serializable for FencingRequest {
    fn serialize(&self, sink: &mut impl FnMut(&[u8])) {
        tl::functions::Ping { ping_id: 1 }.serialize(sink);
    }
}
impl tl::RemoteCall for FencingRequest {
    type Return = FencingResult;
}

#[tokio::test]
async fn a_session_fenced_during_response_decoding_cannot_publish_success() {
    let (_directory, session) = session().await;
    *FENCE_SESSION.lock().unwrap() = Some(session.clone());
    let client = fixture(session, ClientOptions::default(), |_, bytes| async move {
        Ok(pong(&bytes))
    });
    let result = client.invoke(&FencingRequest).await;
    FENCE_SESSION.lock().unwrap().take();
    let failure = result.unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Session);
    assert_eq!(failure.outcome(), Outcome::Unknown);
    assert_eq!(
        client.close().await.unwrap_err().kind(),
        ClientErrorKind::Session
    );
}
