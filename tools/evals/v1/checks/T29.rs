use crate::fixture::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::Relaxed},
};
use tg_mtproto::{ClientOptions, test_kit, tl};

#[tokio::test]
async fn uses_typed_ping_and_returns_actual_pong_from_synthetic_transport() {
    for ping_id in [-555_183, 830] {
        let (_dir, session) = mt_session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = test_kit::client(session, ClientOptions::default(), move |_, wire| {
            seen.fetch_add(1, Relaxed);
            async move { Ok(pong(&wire, ping_id, 105_981)) }
        })
        .unwrap();
        match solution::solve(&client, ping_id).await.unwrap() {
            tl::enums::Pong::Pong(pong) => {
                assert_eq!((pong.ping_id, pong.msg_id), (ping_id, 105_981))
            }
        }
        assert_eq!(calls.load(Relaxed), 1);
        client.close().await.unwrap();
    }
}
