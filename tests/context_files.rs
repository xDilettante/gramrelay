#![cfg(feature = "bot")]
use serde_json::json;
use tg::{
    Bot, CallbackContext, EffectOutcome, InputFile, MessageContext, MessageEvent, Router, Update,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

fn bot(server: &MockServer) -> Bot {
    Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}
fn response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":-123,"type":"supergroup"}}}))
}
fn message() -> serde_json::Value {
    json!({"message_id":9,"date":1,"chat":{"id":-123,"type":"supergroup"},"message_thread_id":2,"business_connection_id":"business","direct_messages_topic":{"topic_id":3}})
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
async fn document_reply_keeps_every_delivery_field_and_streams_the_file() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendDocument"))
        .respond_with(response())
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.bin");
    tokio::fs::write(&file, b"disk content").await.unwrap();
    let router = Router::with_state(file).on_message_event(
        MessageEvent::BusinessMessage,
        |ctx: MessageContext<std::path::PathBuf>| async move {
            ctx.reply_document(InputFile::path(ctx.state()))
                .caption("report")
                .await?;
            Ok(())
        },
    );
    router
        .dispatch(
            &bot(&server),
            serde_json::from_value(json!({"update_id":1,"business_message":message()})).unwrap(),
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body);
    for value in [
        "name=\"chat_id\"\r\n\r\n-123",
        "name=\"message_thread_id\"\r\n\r\n2",
        "name=\"direct_messages_topic_id\"\r\n\r\n3",
        "name=\"business_connection_id\"\r\n\r\nbusiness",
        "{\"message_id\":9}",
        "disk content",
    ] {
        assert!(body.contains(value), "missing {value}");
    }
}

#[tokio::test]
async fn callback_document_replies_are_explicit_about_normal_and_ephemeral_targets() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendDocument"))
        .respond_with(response())
        .expect(2)
        .mount(&server)
        .await;
    let normal = Router::new().on_callback(|ctx: CallbackContext| async move {
        ctx.reply_document(InputFile::bytes("a", vec![1]))?.await?;
        Ok(())
    });
    normal
        .dispatch(&bot(&server), callback(Some(message())))
        .await
        .unwrap();
    let ephemeral = Router::new().on_callback(|ctx: CallbackContext| async move {
        assert_eq!(
            ctx.reply_document(InputFile::path("missing"))
                .unwrap_err()
                .outcome(),
            EffectOutcome::NotSent
        );
        ctx.ephemeral_reply_document(InputFile::bytes("b", vec![2]))?
            .await?;
        Ok(())
    });
    ephemeral
        .dispatch(&bot(&server), callback(None))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[1].body);
    assert!(body.contains("{\"callback_query_id\":\"cb\"}"));
    assert!(!body.contains("name=\"chat_id\""));
}

#[tokio::test]
async fn invalid_contexts_fail_before_opening_or_uploading_a_file() {
    let server = MockServer::start().await;
    let guest = Router::new().on_message_event(
        MessageEvent::GuestMessage,
        |ctx: MessageContext| async move {
            let error = ctx
                .reply_document(InputFile::path("missing"))
                .await
                .unwrap_err();
            assert_eq!(error.field(), Some("guest_query_id"));
            assert_eq!(error.outcome(), EffectOutcome::NotSent);
            Ok(())
        },
    );
    guest
        .dispatch(
            &bot(&server),
            serde_json::from_value(json!({"update_id":1,"guest_message":message()})).unwrap(),
        )
        .await
        .unwrap();
    let inaccessible = Router::new().on_callback(|ctx: CallbackContext| async move {
        assert_eq!(
            ctx.reply_document(InputFile::path("missing"))
                .unwrap_err()
                .outcome(),
            EffectOutcome::NotSent
        );
        Ok(())
    });
    inaccessible
        .dispatch(
            &bot(&server),
            callback(Some(
                json!({"message_id":1,"date":0,"chat":{"id":1,"type":"private"}}),
            )),
        )
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn ephemeral_document_reply_retains_the_receiver() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendDocument"))
        .respond_with(response())
        .expect(1)
        .mount(&server)
        .await;
    let mut msg = message();
    msg["message_id"] = 0.into();
    msg["ephemeral_message_id"] = 7.into();
    msg["receiver_user"] = json!({"id":5,"is_bot":false,"first_name":"User"});
    msg["date"] = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .into();
    let router = Router::new().on_message(|ctx: MessageContext| async move {
        ctx.reply_document(InputFile::bytes("p", vec![1])).await?;
        Ok(())
    });
    router
        .dispatch(
            &bot(&server),
            serde_json::from_value(json!({"update_id":1,"message":msg})).unwrap(),
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("{\"receiver_user_id\":5}"));
    assert!(body.contains("{\"ephemeral_message_id\":7}"));
}

#[tokio::test]
async fn the_remaining_ephemeral_window_bounds_upload_and_response_time() {
    let server = MockServer::start().await;
    Mock::given(path("/bot123456:OFFLINE_TEST_TOKEN/sendDocument"))
        .respond_with(response().set_delay(std::time::Duration::from_secs(5)))
        .expect(1)
        .mount(&server)
        .await;
    let mut msg = message();
    msg["ephemeral_message_id"] = 7.into();
    msg["receiver_user"] = json!({"id":5,"is_bot":false,"first_name":"User"});
    msg["date"] = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 13)
        .into();
    let router = Router::new().on_message(|ctx: MessageContext| async move {
        let started = std::time::Instant::now();
        let error = ctx
            .reply_document(InputFile::bytes("p", vec![1]))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), tg::ErrorKind::Timeout);
        assert_eq!(error.outcome(), EffectOutcome::Unknown);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        Ok(())
    });
    router
        .dispatch(
            &bot(&server),
            serde_json::from_value(json!({"update_id":1,"message":msg})).unwrap(),
        )
        .await
        .unwrap();
}
