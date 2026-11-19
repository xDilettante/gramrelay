use serde_json::json;
use std::{error::Error as _, sync::Arc, time::Duration};
use tg::{Bot, ChatId, EffectOutcome, ErrorKind, InputFile};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::method};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";

fn bot(server: &MockServer) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}

#[tokio::test]
async fn explicit_refusal_exposes_parameters_but_not_private_description() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "ok":false,"error_code":429,"description":format!("Private body and token {TOKEN}"),
            "parameters":{"retry_after":3,"migrate_to_chat_id":-100789}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let error = bot(&server)
        .send_message(ChatId::from(1), "Sensitive message")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Telegram);
    assert_eq!(error.outcome(), EffectOutcome::Rejected);
    assert_eq!(error.http_status(), Some(429));
    assert_eq!(error.api_error_code(), Some(429));
    assert_eq!(error.retry_after(), Some(Duration::from_secs(3)));
    assert_eq!(error.migrate_to_chat_id(), Some(ChatId::from(-100789)));
    assert!(error.source().is_none());
    let diagnostic = format!("{error:?} {error}");
    for private in [TOKEN, "Private body", "Sensitive message"] {
        assert!(!diagnostic.contains(private));
    }
}

#[tokio::test]
async fn ambiguous_responses_remain_unknown_without_retry() {
    for response in [
        ResponseTemplate::new(500).set_body_json(json!({"ok":false,"error_code":500})),
        ResponseTemplate::new(200).set_body_string("not JSON"),
        ResponseTemplate::new(200).set_body_json(json!({"ok":true})),
        ResponseTemplate::new(200)
            .set_body_json(json!({"ok":true,"result":{"message_id":"invalid"}})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = bot(&server)
            .send_message(ChatId::from(1), "Once")
            .await
            .unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::Unknown);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn oversized_response_is_bounded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(1024)))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_response_bytes(128)
        .build()
        .unwrap();
    let error = bot.send_message(ChatId::from(1), "Once").await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ResponseTooLarge);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}

#[tokio::test]
async fn redirects_do_not_forward_the_token_or_request() {
    let source = MockServer::start().await;
    let target = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", target.uri()))
        .expect(1)
        .mount(&source)
        .await;
    let error = bot(&source)
        .send_message(ChatId::from(1), "Private")
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn overload_is_not_sent_and_timeout_releases_the_permit() {
    let server = MockServer::start().await;
    let received = Arc::new(Notify::new());
    let signal = received.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            signal.notify_one();
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(json!({"ok":true}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(500))
        .max_in_flight(1)
        .build()
        .unwrap();
    let first_bot = bot.clone();
    let first = tokio::spawn(async move { first_bot.send_message(ChatId::from(1), "First").await });
    tokio::time::timeout(Duration::from_secs(2), received.notified())
        .await
        .unwrap();
    let error = bot
        .send_message(ChatId::from(1), "Overloaded")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Capacity);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert_eq!(first.await.unwrap().unwrap_err().kind(), ErrorKind::Timeout);
    let next = bot
        .send_message(ChatId::from(1), "After timeout")
        .await
        .unwrap_err();
    assert_eq!(next.kind(), ErrorKind::Timeout);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn document_timeout_does_not_reupload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
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
        .send_document(
            ChatId::from(1),
            InputFile::bytes("report.bin", b"data".to_vec()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn deadline_covers_a_stalled_response_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n{\"ok\":")
            .await
            .unwrap();
        stream.flush().await.unwrap();
        std::future::pending::<()>().await;
    });
    let bot = Bot::builder(TOKEN)
        .api_url(url)
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    let error = bot.send_message(ChatId::from(1), "Once").await.unwrap_err();
    server.abort();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}

#[test]
fn unsafe_transport_and_invalid_limits_fail_without_leaking_input() {
    for builder in [
        Bot::builder(TOKEN).api_url("http://api.telegram.org/"),
        Bot::builder(TOKEN)
            .api_url("http://example.com/")
            .allow_http_loopback(true),
        Bot::builder(TOKEN).api_url("https://user:password@example.com/"),
        Bot::builder(TOKEN).api_url("https://example.com/?private=true"),
        Bot::builder(TOKEN).api_url("https://example.com/#fragment"),
        Bot::builder(TOKEN).timeout(Duration::ZERO),
        Bot::builder(TOKEN).timeout(Duration::MAX),
        Bot::builder(TOKEN).max_in_flight(0),
        Bot::builder(TOKEN).max_response_bytes(0),
        Bot::builder(TOKEN).max_upload_bytes(0),
        Bot::builder(format!("{TOKEN}/escape")),
    ] {
        let error = builder.build().unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
        assert!(!format!("{error:?} {error}").contains("password"));
    }
}

#[tokio::test]
async fn dropping_an_unawaited_request_has_no_effect() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    drop(bot.send_message(ChatId::from(1), "Lazy"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_file_name_is_not_sent() {
    let server = MockServer::start().await;
    let error = bot(&server)
        .send_document(ChatId::from(1), InputFile::bytes("bad\r\nname", vec![1]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Validation);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn chunked_response_without_content_length_is_also_bounded() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let response = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n81\r\n{}\r\n0\r\n\r\n",
            "x".repeat(129)
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    });
    let bot = Bot::builder(TOKEN)
        .api_url(url)
        .allow_http_loopback(true)
        .max_response_bytes(128)
        .build()
        .unwrap();
    let error = bot.send_message(ChatId::from(1), "Once").await.unwrap_err();
    server.await.unwrap();
    assert_eq!(error.kind(), ErrorKind::ResponseTooLarge);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
}
