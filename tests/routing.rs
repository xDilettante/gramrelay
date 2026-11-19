#![cfg(feature = "bot")]

use serde_json::json;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tg::{Bot, CallbackContext, EffectOutcome, ErrorKind, MessageContext, Router, Update};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path},
};

fn update(text: &str) -> Update {
    serde_json::from_value(json!({"update_id":1,"message":{
        "message_id":77,"date":1,"chat":{"id":-100123,"type":"supergroup"},"text":text,
        "message_thread_id":12,"business_connection_id":"business-1"
    }}))
    .unwrap()
}

#[tokio::test]
async fn command_context_preserves_reply_thread_business_and_state() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendMessage"))
        .and(body_json(
            json!({"chat_id":-100123,"text":"Hello","reply_parameters":{"message_id":77},
            "message_thread_id":12,"business_connection_id":"business-1"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
                "message_id":78,"date":1,"chat":{"id":-100123,"type":"supergroup"}
            }})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap();
    let router = Router::with_state(Mutex::new(Vec::<String>::new()))
        .bot_username("demo_bot")
        .command(
            "start",
            |ctx: MessageContext<Mutex<Vec<String>>>| async move {
                ctx.state()
                    .lock()
                    .unwrap()
                    .push(ctx.command_args().unwrap().to_owned());
                ctx.reply("Hello").await?;
                Ok(())
            },
        );
    router
        .dispatch(&bot, update("/start@demo_bot  first second"))
        .await
        .unwrap();
    assert_eq!(*router.state().lock().unwrap(), vec!["first second"]);
}

#[tokio::test]
async fn command_names_and_bot_mentions_match_exactly() {
    let bot = Bot::new("123456:OFFLINE_TEST_TOKEN").unwrap();
    let router = Router::with_state(AtomicUsize::new(0))
        .bot_username("demo_bot")
        .command("start", |ctx: MessageContext<AtomicUsize>| async move {
            ctx.state().fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    for text in [
        "/start",
        "/start@demo_bot",
        "/start@DEMO_BOT",
        "/start@other_bot",
        "/starting",
        " /start",
        "plain text",
    ] {
        router.dispatch(&bot, update(text)).await.unwrap();
    }
    assert_eq!(router.state().load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn callback_context_answers_exact_data() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/answerCallbackQuery"))
        .and(body_json(
            json!({"callback_query_id":"query-1","text":"Received"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":true})))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap();
    let router = Router::new().callback("confirm", |ctx: CallbackContext| async move {
        ctx.answer().text("Received").await?;
        Ok(())
    });
    let event = serde_json::from_value(json!({"update_id":2,"callback_query":{
        "id":"query-1","from":{"id":1,"is_bot":false,"first_name":"User"},
        "chat_instance":"instance","data":"confirm"
    }}))
    .unwrap();
    router.dispatch(&bot, event).await.unwrap();
}

#[tokio::test]
async fn first_matching_route_runs_once_and_bad_registration_is_local() {
    let bot = Bot::new("123456:OFFLINE_TEST_TOKEN").unwrap();
    let router = Router::with_state(AtomicUsize::new(0))
        .command("start", |ctx: MessageContext<AtomicUsize>| async move {
            ctx.state().fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .on_message(|ctx: MessageContext<AtomicUsize>| async move {
            ctx.state().fetch_add(10, Ordering::SeqCst);
            Ok(())
        });
    router.dispatch(&bot, update("/start")).await.unwrap();
    assert_eq!(router.state().load(Ordering::SeqCst), 1);
    let invalid = Router::new().command("/invalid", |_ctx: MessageContext| async { Ok(()) });
    let error = invalid.dispatch(&bot, update("/start")).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Validation);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
}
