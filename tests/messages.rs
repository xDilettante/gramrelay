use serde_json::json;
use std::time::Duration;
use tg::{
    Bot, ButtonStyle, ChatId, EffectOutcome, ErrorKind, InlineKeyboardButton, InlineKeyboardMarkup,
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
    assert_eq!(i64::from(result.message_id), 42);
    assert_eq!(result.chat.id, ChatId::from(-100123));
    assert_eq!(result.text.as_deref(), Some("Привет 👋"));
}

#[tokio::test]
async fn typed_keyboard_sends_callback_links_and_copy_buttons_with_styles() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/sendMessage")))
        .and(body_json(
            json!({"chat_id":1,"text":"Options","reply_markup":{"inline_keyboard":[
                [{"text":"Confirm","callback_data":"confirm"}],
                [{"text":"Docs","url":"https://example.com/","style":"primary"},
                 {"text":"Copy","copy_text":{"text":"code"},"style":"success"}]
            ]}}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
                "message_id":42,"date":1,"chat":{"id":1,"type":"private"}
            }})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let keyboard: tg::api::types::InlineKeyboardMarkup = InlineKeyboardMarkup::new()
        .row([InlineKeyboardButton::callback("Confirm", "confirm")])
        .row([
            InlineKeyboardButton::url("Docs", "https://example.com/").style(ButtonStyle::Primary),
            InlineKeyboardButton::copy_text("Copy", "code").style(ButtonStyle::Success),
        ])
        .into();
    bot(&server)
        .send_message(ChatId::from(1), "Options")
        .reply_markup(keyboard.into())
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_keyboard_actions_are_rejected_before_dispatch() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    for (button, field) in [
        (
            InlineKeyboardButton::callback("Invalid", "я".repeat(33)),
            "callback_data",
        ),
        (
            InlineKeyboardButton::url("Invalid", "relative.html"),
            "button_url",
        ),
        (InlineKeyboardButton::copy_text("Invalid", ""), "copy_text"),
        (
            tg::api::types::InlineKeyboardButton::new("Invalid".to_owned()).into(),
            "button_action",
        ),
    ] {
        let error = bot
            .send_message(ChatId::from(1), "Options")
            .reply_markup(InlineKeyboardMarkup::new().row([button]))
            .await
            .unwrap_err();
        assert_eq!(error.field(), Some(field));
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
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
