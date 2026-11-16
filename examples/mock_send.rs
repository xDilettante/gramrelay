use serde_json::json;
use tg::{Bot, ChatId};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot123456:OFFLINE_TEST_TOKEN/sendMessage"))
        .and(body_json(
            json!({"chat_id":1,"text":"Hello from a local mock"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()?;
    let sent = bot
        .send_message(ChatId::from(1), "Hello from a local mock")
        .await?;
    assert_eq!(i64::from(sent.message_id), 42);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    println!(
        "Local mock passed: message_id={}",
        i64::from(sent.message_id)
    );
    Ok(())
}
