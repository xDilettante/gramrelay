use crate::fixture::*;
use serde_json::json;
use std::{
    sync::Arc,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
    time::Duration,
};
use tg::{CancellationToken, Router, test_kit::MockTransport, webhook::WebhookSecret};

#[tokio::test]
async fn loopback_requires_secret_and_limits_body_before_dispatch() {
    let mock = MockTransport::new([reply(
        "getMe",
        json!({"id":0,"is_bot":true,"first_name":"fixture","username":"fixture_bot"}),
    )])
    .unwrap();
    let bot = mock.bot().unwrap();
    let stop = CancellationToken::new();
    let count = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state(count.clone()).on_message(|ctx| async move {
        ctx.state().fetch_add(1, SeqCst);
        Ok(())
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/telegram", listener.local_addr().unwrap());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let proper = "EVAL_FIXTURE_SECRET";
    let header = "x-telegram-bot-api-secret-token";
    let payload = json!({"update_id":91,"message":{"message_id":7,"date":1,"chat":{"id":21,"type":"private"},"text":"ready"}});
    let client_flow = async {
        let send = |token: Option<&str>, data: serde_json::Value| {
            let request = client.post(&url).json(&data);
            match token {
                Some(token) => request.header(header, token),
                None => request,
            }
            .send()
        };
        for token in [None, Some("incorrect")] {
            assert_eq!(send(token, payload.clone()).await.unwrap().status(), 401);
        }
        let mut large = payload.clone();
        large["message"]["text"] = json!("a".repeat(512));
        assert_eq!(send(Some(proper), large).await.unwrap().status(), 413);
        assert_eq!(count.load(SeqCst), 0);
        assert_eq!(send(Some(proper), payload).await.unwrap().status(), 200);
        stop.cancel();
    };
    let (_, result) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            client_flow,
            solution::solve(
                &bot,
                router,
                WebhookSecret::new(proper).unwrap(),
                listener,
                256,
                stop.clone()
            )
        )
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(count.load(SeqCst), 1);
    mock.assert_finished().unwrap();
}
