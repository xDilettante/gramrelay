use crate::fixture::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    time::Duration,
};
use tg_mtproto::{ClientOptions, Outcome, test_kit, tl};
use tokio::sync::Notify;

struct Active(Arc<AtomicBool>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.store(false, SeqCst);
    }
}

#[tokio::test]
async fn closes_the_driver_and_releases_session_before_returning() {
    let (_dir, session) = mt_session().await;
    let active = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(Notify::new());
    let observed = active.clone();
    let started = ready.clone();
    let client = test_kit::client(session.clone(), ClientOptions::default(), move |_, wire| {
        let active = observed.clone();
        let ready = started.clone();
        async move {
            let _ = pong(&wire, 611, 7);
            active.store(true, SeqCst);
            let _guard = Active(active);
            ready.notify_one();
            std::future::pending().await
        }
    })
    .unwrap();
    let worker = {
        let client = client.clone();
        tokio::spawn(async move { client.invoke(&tl::functions::Ping { ping_id: 611 }).await })
    };
    tokio::time::timeout(Duration::from_secs(4), ready.notified())
        .await
        .unwrap();
    assert!(active.load(SeqCst));
    solution::solve(&client).await.unwrap();
    assert!(!active.load(SeqCst));
    let _ = worker.await.unwrap().unwrap_err();
    assert_eq!(
        client
            .invoke(&tl::functions::Ping { ping_id: 7 })
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    let replacement = test_kit::client(session, ClientOptions::default(), |_, wire| async move {
        Ok(pong(&wire, 99, 8))
    })
    .unwrap();
    replacement
        .invoke(&tl::functions::Ping { ping_id: 99 })
        .await
        .unwrap();
    replacement.close().await.unwrap();
}
