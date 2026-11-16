use serde_json::json;
use tg::{Bot, EffectOutcome, ErrorKind};
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
async fn fetches_typed_updates_with_explicit_parameters() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path(format!("/bot{TOKEN}/getUpdates")))
        .and(body_json(json!({"offset":42,"limit":10,"timeout":20,"allowed_updates":["message","callback_query"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[{
            "update_id":42,"message":{"message_id":5,"date":1,"chat":{"id":1,"type":"private","future_chat":"retained"},
            "text":"/start","future_message":{"value":1}}
        }]}))).expect(1).mount(&server).await;
    let updates = bot(&server)
        .get_updates()
        .offset(42)
        .limit(10)
        .timeout(20)
        .allowed_updates(["message", "callback_query"])
        .await
        .unwrap();
    assert_eq!(updates[0].update_id, 42);
    let message = updates[0].message.as_ref().unwrap();
    assert_eq!(message.unknown_fields["future_message"]["value"], 1);
    assert_eq!(message.chat.unknown_fields["future_chat"], "retained");
    let saved = serde_json::to_value(&updates[0]).unwrap();
    assert_eq!(saved["message"]["future_message"]["value"], 1);
}

#[tokio::test]
async fn identity_and_webhook_info_use_typed_results() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/bot{TOKEN}/getMe")))
        .and(body_json(json!({})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
                "id":123456,"is_bot":true,"first_name":"Demo","username":"demo_bot"
            }})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(format!("/bot{TOKEN}/getWebhookInfo")))
        .and(body_json(json!({})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
                "url":"","has_custom_certificate":false,"pending_update_count":7
            }})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let bot = bot(&server);
    assert_eq!(
        bot.get_me().await.unwrap().username.as_deref(),
        Some("demo_bot")
    );
    assert_eq!(
        bot.get_webhook_info().await.unwrap().pending_update_count,
        7
    );
}

#[tokio::test]
async fn invalid_poll_parameters_do_not_discard_or_fetch_updates() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    for request in [
        bot.get_updates().offset(-1),
        bot.get_updates().limit(0),
        bot.get_updates().limit(101),
    ] {
        let error = request.await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_update_with_two_known_event_kinds_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":[{
            "update_id":1,
            "message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}},
            "callback_query":{"id":"query","from":{"id":1,"is_bot":false,"first_name":"User"},"chat_instance":"instance"}
        }]}))).expect(1).mount(&server).await;
    let error = bot(&server).get_updates().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Decode);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}
