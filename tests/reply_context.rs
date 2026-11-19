#![cfg(feature = "bot")]
use serde_json::json;
use tg::{Bot, CallbackContext, EffectOutcome, MessageContext, MessageEvent, Router, Update};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path},
};

fn bot(server: &MockServer) -> Bot {
    Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}
fn result() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":-123,"type":"supergroup"}}}))
}
fn message() -> serde_json::Value {
    json!({"message_id":10,"date":1,"chat":{"id":-123,"type":"supergroup"},"text":"hello",
        "message_thread_id":3,"business_connection_id":"business","direct_messages_topic":{"topic_id":7}})
}
fn callback(message: Option<serde_json::Value>) -> Update {
    let mut query =
        json!({"id":"cb","from":{"id":1,"is_bot":false,"first_name":"User"},"chat_instance":"i"});
    if let Some(message) = message {
        query["message"] = message;
    }
    serde_json::from_value(json!({"update_id":1,"callback_query":query})).unwrap()
}

#[tokio::test]
async fn business_ephemeral_parent_preserves_every_available_reply_scope() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendMessage"))
        .and(body_json(json!({"chat_id":-123,"text":"reply","message_thread_id":3,"business_connection_id":"business",
            "direct_messages_topic_id":7,"reply_parameters":{"ephemeral_message_id":55},
            "ephemeral_message_parameters":{"receiver_user_id":9}})))
        .respond_with(result()).expect(1).mount(&server).await;
    let mut message = message();
    message["message_id"] = 0.into();
    message["ephemeral_message_id"] = 55.into();
    message["receiver_user"] = json!({"id":9,"is_bot":false,"first_name":"Receiver"});
    message["date"] = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .into();
    let update = serde_json::from_value(json!({"update_id":1,"business_message":message})).unwrap();
    let router = Router::new().on_message_event(
        MessageEvent::BusinessMessage,
        |ctx: MessageContext| async move {
            assert_eq!(ctx.event, MessageEvent::BusinessMessage);
            ctx.reply("reply").await?;
            Ok(())
        },
    );
    router.dispatch(&bot(&server), update).await.unwrap();
}

#[tokio::test]
async fn accessible_callback_reply_keeps_the_message_context() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendMessage"))
        .and(body_json(json!({"chat_id":-123,"text":"reply","message_thread_id":3,"business_connection_id":"business",
            "direct_messages_topic_id":7,"reply_parameters":{"message_id":10}})))
        .respond_with(result()).expect(1).mount(&server).await;
    let router = Router::new().on_callback(|ctx: CallbackContext| async move {
        ctx.reply("reply")?.await?;
        Ok(())
    });
    router
        .dispatch(&bot(&server), callback(Some(message())))
        .await
        .unwrap();
}

#[tokio::test]
async fn inline_callback_can_reply_ephemerally_without_inventing_a_chat() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendMessage"))
        .and(body_json(
            json!({"text":"only caller","ephemeral_message_parameters":{"callback_query_id":"cb"}}),
        ))
        .respond_with(result())
        .expect(1)
        .mount(&server)
        .await;
    let router = Router::new().on_callback(|ctx: CallbackContext| async move {
        assert_eq!(
            ctx.reply("bad").unwrap_err().outcome(),
            EffectOutcome::NotSent
        );
        ctx.ephemeral_reply("only caller")?.await?;
        Ok(())
    });
    router
        .dispatch(&bot(&server), callback(None))
        .await
        .unwrap();
}

#[tokio::test]
async fn inaccessible_or_unsupported_contexts_never_fall_back_to_normal_sends() {
    let server = MockServer::start().await;
    let router = Router::new().on_callback(|ctx: CallbackContext| async move {
        assert_eq!(
            ctx.reply("bad").unwrap_err().outcome(),
            EffectOutcome::NotSent
        );
        Ok(())
    });
    router
        .dispatch(
            &bot(&server),
            callback(Some(
                json!({"message_id":1,"date":0,"chat":{"id":-123,"type":"group"}}),
            )),
        )
        .await
        .unwrap();
    let router = Router::new().on_callback(|ctx: CallbackContext| async move {
        assert_eq!(
            ctx.ephemeral_reply("bad").unwrap_err().field(),
            Some("ephemeral_chat")
        );
        Ok(())
    });
    let mut message = message();
    message["chat"]["is_direct_messages"] = true.into();
    router
        .dispatch(&bot(&server), callback(Some(message)))
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn guest_reply_uses_the_guest_query_operation() {
    use tg::api::types::{InlineQueryResultArticle, InputTextMessageContent};
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/answerGuestQuery"))
        .and(body_json(
            json!({"guest_query_id":"guest","result":{"type":"article","id":"1","title":"Reply",
            "input_message_content":{"message_text":"guest reply"}}}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok":true,"result":{"inline_message_id":"sent"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut message = message();
    message["guest_query_id"] = "guest".into();
    let update = serde_json::from_value(json!({"update_id":1,"guest_message":message})).unwrap();
    let router = Router::new().on_message_event(
        MessageEvent::GuestMessage,
        |ctx: MessageContext| async move {
            let error = ctx.reply("wrong operation").await.unwrap_err();
            assert_eq!(error.field(), Some("guest_query_id"));
            let result = InlineQueryResultArticle::new(
                "1".into(),
                "Reply".into(),
                InputTextMessageContent::new("guest reply".into()).into(),
            );
            ctx.answer_guest(result.into())?.await?;
            Ok(())
        },
    );
    router.dispatch(&bot(&server), update).await.unwrap();
}

#[tokio::test]
async fn an_ephemeral_parent_never_loses_its_receiver_or_reuses_an_expired_id() {
    let server = MockServer::start().await;
    for (receiver, expected) in [
        (false, "receiver_user_id"),
        (true, "ephemeral_message_expired"),
    ] {
        let mut message = message();
        message["ephemeral_message_id"] = 1.into();
        if receiver {
            message["receiver_user"] = json!({"id":9,"is_bot":false,"first_name":"User"});
        }
        let update = serde_json::from_value(json!({"update_id":1,"message":message})).unwrap();
        let router = Router::new().on_message(move |ctx: MessageContext| async move {
            let error = ctx.reply("private").await.unwrap_err();
            assert_eq!(error.field(), Some(expected));
            assert_eq!(error.outcome(), EffectOutcome::NotSent);
            Ok(())
        });
        router.dispatch(&bot(&server), update).await.unwrap();
    }
    let error = bot(&server)
        .send_message(tg::ChatId::from(1), "private")
        .reply_to_ephemeral(1)
        .await
        .unwrap_err();
    assert_eq!(error.field(), Some("reply_parameters"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_guest_event_without_a_query_id_cannot_fall_back_to_its_numeric_chat_id() {
    let server = MockServer::start().await;
    let update = serde_json::from_value(json!({"update_id":1,"guest_message":message()})).unwrap();
    let router = Router::new().on_message_event(
        MessageEvent::GuestMessage,
        |ctx: MessageContext| async move {
            let error = ctx
                .reply("must not leak into another chat")
                .await
                .unwrap_err();
            assert_eq!(error.field(), Some("guest_query_id"));
            assert_eq!(error.outcome(), EffectOutcome::NotSent);
            Ok(())
        },
    );
    router.dispatch(&bot(&server), update).await.unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
}
