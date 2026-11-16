use serde_json::json;
use std::time::Duration;
use tg::{
    Bot, ChatId, EffectOutcome, ErrorKind, UserId,
    api::{Effect, Method, methods},
};
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
async fn complete_api_is_lazy_and_uses_the_shared_transport() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path(format!("/bot{TOKEN}/sendMessage")))
        .and(body_json(json!({"text": "Привет 🦀", "chat_id": -123, "protect_content": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": {"message_id": 1, "date": 1, "chat": {"id": -123, "type": "group"}, "new_field": 42}})))
        .expect(1).mount(&server).await;
    let bot = bot(&server);
    let call = bot
        .api()
        .send_message("Привет 🦀".into())
        .chat_id(ChatId::from(-123).into())
        .protect_content(true);
    assert!(server.received_requests().await.unwrap().is_empty());
    let response = call.await.unwrap();
    assert_eq!(response.unknown_fields()["new_field"], 42);
    assert_eq!(i64::from(response.message_id), 1);
}

#[tokio::test]
async fn generated_callback_and_10_4_method_are_callable() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/bot{TOKEN}/answerCallbackQuery")))
        .and(body_json(json!({"callback_query_id": "cb", "text": "OK"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(format!("/bot{TOKEN}/sendWidget")))
        .and(body_json(json!({"html": "<b>Widget</b>", "chat_id": 1})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": {"message_id": 2, "date": 1, "chat": {"id": 1, "type": "private"}}})))
        .expect(1).mount(&server).await;
    let bot = bot(&server);
    bot.call(methods::AnswerCallbackQuery::new("cb".into()).text("OK".into()))
        .await
        .unwrap();
    bot.api()
        .send_widget(ChatId::from(1).into(), "<b>Widget</b>".into())
        .await
        .unwrap();
}

#[tokio::test]
async fn generated_write_timeout_is_unknown_and_never_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let error = bot
        .api()
        .delete_message(ChatId::from(1).into(), 7.into())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(!format!("{error:?}").contains(TOKEN));
}

#[tokio::test]
async fn credential_results_require_explicit_exposure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "result": "999:SYNTHETIC_SECRET"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let secret = bot(&server)
        .api()
        .get_managed_bot_token(UserId::from(7))
        .await
        .unwrap();
    assert_eq!(secret.expose_secret(), "999:SYNTHETIC_SECRET");
    assert!(!format!("{secret:?}").contains("SYNTHETIC_SECRET"));
}

#[test]
fn effects_are_reviewed_and_parameters_reject_unknown_fields() {
    assert_eq!(methods::GetMe::EFFECT, Effect::Read);
    assert_eq!(methods::GetUpdates::EFFECT, Effect::Acknowledge);
    assert_eq!(methods::ReadBusinessMessage::EFFECT, Effect::Write);
    assert_eq!(methods::ExportChatInviteLink::EFFECT, Effect::Write);
    assert_eq!(methods::GetManagedBotToken::EFFECT, Effect::CredentialRead);
    assert!(
        serde_json::from_value::<methods::SendMessage>(
            json!({"text": "x", "raw_call": "deleteMessage"})
        )
        .is_err()
    );
}
