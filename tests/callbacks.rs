use serde_json::json;
use tg::{
    Bot, ChatId, EffectOutcome, ErrorKind, InlineKeyboardButton, InlineKeyboardMarkup, Update,
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
async fn sends_a_callback_button_and_answers_the_received_query() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/bot{TOKEN}/sendMessage")))
        .and(body_json(json!({"chat_id":1,"text":"Confirm?","reply_markup":{"inline_keyboard":[[{"text":"Confirm","callback_data":"confirm"}]]}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}})))
        .expect(1).mount(&server).await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/answerCallbackQuery")))
        .and(body_json(
            json!({"callback_query_id":"query-42","text":"Confirmed","show_alert":true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":true})))
        .expect(1)
        .mount(&server)
        .await;
    let bot = bot(&server);
    bot.send_message(ChatId::from(1), "Confirm?")
        .reply_markup(
            InlineKeyboardMarkup::new().row([InlineKeyboardButton::callback("Confirm", "confirm")]),
        )
        .await
        .unwrap();
    let update: Update = serde_json::from_value(json!({"update_id":10,"callback_query":{"id":"query-42","from":{"id":7,"is_bot":false,"first_name":"User"},"chat_instance":"instance","data":"confirm"}})).unwrap();
    let query = update.callback_query.unwrap();
    assert_eq!(query.data.as_deref(), Some("confirm"));
    assert!(
        bot.answer_callback_query(query.id)
            .text("Confirmed")
            .show_alert(true)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn callback_data_limit_counts_bytes_not_characters() {
    let server = MockServer::start().await;
    let error = bot(&server)
        .send_message(ChatId::from(1), "Choose")
        .reply_markup(
            InlineKeyboardMarkup::new()
                .row([InlineKeyboardButton::callback("Too long", "я".repeat(33))]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Validation);
    assert_eq!(error.field(), Some("callback_data"));
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_callback_reply_is_not_sent() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    for request in [
        bot.answer_callback_query("".into()),
        bot.answer_callback_query("valid-id".into())
            .text("x".repeat(201)),
    ] {
        let error = request.await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn unknown_update_fields_survive_but_malformed_callbacks_are_rejected() {
    let future = json!({"update_id":10,"future_event":{"value":42}});
    let update: Update = serde_json::from_value(future).unwrap();
    assert_eq!(update.unknown_fields["future_event"]["value"], 42);
    assert!(
        serde_json::from_value::<Update>(
            json!({"update_id":11,"callback_query":{"id":"missing-required-fields"}})
        )
        .is_err()
    );
}
