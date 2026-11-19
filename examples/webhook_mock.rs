//! Real localhost HTTP intake, synthetic identity, no Telegram network requests.
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tg::{
    CancellationToken, Router,
    test_kit::{MockResponse, MockTransport, Step},
    webhook::WebhookSecret,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mock = MockTransport::new([Step::reply(
        "getMe",
        MockResponse::json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline","username":"offline_bot"}}),
        ),
    )])?;
    let bot = mock.bot()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state(calls.clone()).command("start", |ctx| async move {
        ctx.state().fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let secret = WebhookSecret::new("SYNTHETIC_LOCAL_SECRET")?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/telegram", listener.local_addr()?);
    let stop = CancellationToken::new();
    let serving = {
        let stop = stop.clone();
        let secret = secret.clone();
        tokio::spawn(async move {
            bot.webhook(router, secret)
                .serve_until(listener, stop)
                .await
        })
    };
    let client = reqwest::Client::builder().no_proxy().build()?;
    let update = json!({"update_id":1,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"/start@offline_bot","entities":[{"type":"bot_command","offset":0,"length":18}]}});
    for _ in 0..2 {
        let response = client
            .post(&url)
            .header("x-telegram-bot-api-secret-token", secret.expose())
            .json(&update)
            .send()
            .await?;
        assert_eq!(response.status(), 200);
    }
    stop.cancel();
    serving.await??;
    mock.assert_finished()?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    println!("Offline webhook processed one message and deduplicated its redelivery.");
    Ok(())
}
