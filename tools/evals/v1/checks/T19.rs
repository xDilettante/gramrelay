use crate::fixture::*;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
    time::Duration,
};
use tg::{
    CancellationToken, Router,
    test_kit::{MockTransport, Step},
};
use tokio::sync::{Notify, Semaphore};

#[tokio::test]
async fn subscriptions_capacity_and_shutdown_do_not_ack_unfinished_batch() {
    let mock = MockTransport::new([
        reply("getMe", json!({"id":0,"is_bot":true,"first_name":"fixture","username":"fixture_bot"})),
        reply("getWebhookInfo", json!({"url":"","has_custom_certificate":false,"pending_update_count":0})),
        reply("getUpdates", json!([
            {"update_id":14,"message":{"message_id":5,"date":1,"chat":{"id":1,"type":"private"},"text":"one"}},
            {"update_id":15,"message":{"message_id":8,"date":1,"chat":{"id":2,"type":"private"},"text":"two"}}
        ])),
        Step::timeout("getUpdates"),
    ]).unwrap();
    let bot = mock.bot().unwrap();
    let stop = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let permits = Arc::new(Semaphore::new(0));
    let router = Router::with_state((calls.clone(), started.clone(), permits.clone())).on_message(
        |ctx| async move {
            let (calls, started, permits) = ctx.state();
            calls.fetch_add(1, SeqCst);
            started.notify_one();
            permits.acquire().await.unwrap().forget();
            Ok(())
        },
    );
    let controller = async {
        started.notified().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(calls.load(SeqCst), 1);
        stop.cancel();
        permits.add_permits(2);
    };
    let (_, result) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(controller, solution::solve(&bot, router, 1, stop.clone()))
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(calls.load(SeqCst), 2);
    let requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].method(), "getUpdates");
    let body: Value = serde_json::from_slice(requests[2].body()).unwrap();
    let mut kinds: Vec<_> = body["allowed_updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["callback_query", "edited_message", "message"]);
    assert!(body.get("offset").is_none());
}
