use serde_json::json;
use std::time::Duration;
use tg::{Bot, ChatId, EffectOutcome, ErrorKind};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";

fn bot(server: &MockServer) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}

#[tokio::test]
async fn sends_plain_text_and_decodes_a_typed_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/sendMessage")))
        .and(body_json(json!({"chat_id":-100123,"text":"Привет 👋","disable_notification":true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":42,"date":1,"chat":{"id":-100123,"type":"supergroup"},"text":"Привет 👋"}})))
        .expect(1).mount(&server).await;
    let result = bot(&server)
        .send_message(ChatId::from(-100123), "Привет 👋")
        .disable_notification(true)
        .await
        .unwrap();
    assert_eq!(i64::from(result.id), 42);
    assert_eq!(result.chat.id, ChatId::from(-100123));
    assert_eq!(result.text.as_deref(), Some("Привет 👋"));
}

#[tokio::test]
async fn timeout_after_receipt_is_unknown_and_does_not_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(json!({"ok":true,"result":{}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    let error = bot
        .send_message(ChatId::from(1), "Only once")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(!format!("{error:?} {error}").contains(TOKEN));
}

#[tokio::test]
async fn invalid_message_is_not_sent() {
    let server = MockServer::start().await;
    let error = bot(&server)
        .send_message(ChatId::from(1), "")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Validation);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(server.received_requests().await.unwrap().is_empty());
}
