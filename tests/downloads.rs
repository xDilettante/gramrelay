use serde_json::json;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tg::{Bot, EffectOutcome, ErrorKind, api::types::File};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";
fn file(path: &str) -> File {
    File::new("file-id".into(), "unique".into()).file_path(path.into())
}
fn bot(server: &MockServer, limit: u64) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_download_bytes(limit)
        .build()
        .unwrap()
}

#[tokio::test]
async fn downloads_are_lazy_and_use_the_configured_file_endpoint_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/proxy/file/bot{TOKEN}/documents/report.bin")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0, 255, 1, 2]))
        .expect(2)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(format!("{}/proxy/", server.uri()))
        .allow_http_loopback(true)
        .build()
        .unwrap();
    let download = bot.download_file(&file("documents/report.bin"));
    assert!(!format!("{download:?}").contains("report.bin"));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(download.bytes().await.unwrap(), vec![0, 255, 1, 2]);
    let mut output = Vec::new();
    assert_eq!(
        bot.download_file(&file("documents/report.bin"))
            .to_writer(&mut output)
            .await
            .unwrap(),
        4
    );
    assert_eq!(output, vec![0, 255, 1, 2]);
}

#[tokio::test]
async fn paths_and_advertised_sizes_are_checked_without_dispatch() {
    let server = MockServer::start().await;
    let bot = bot(&server, 3);
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a//b",
        "a/./b",
        "a\\b",
        "https://evil.test/a",
        "a?x=1",
        "a#x",
        "a/%2e%2e/b",
        "a/line\n",
    ] {
        let error = bot.download_file(&file(path)).bytes().await.unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
        assert!(!format!("{error:?} {error}").contains("evil.test"));
    }
    for metadata in [
        File::new("id".into(), "u".into()),
        file("a").file_size(-1),
        file("a").file_size(4),
    ] {
        assert_eq!(
            bot.download_file(&metadata)
                .bytes()
                .await
                .unwrap_err()
                .outcome(),
            EffectOutcome::NotSent
        );
    }
    assert_eq!(
        bot.download_file(&file("a"))
            .max_bytes(0)
            .bytes()
            .await
            .unwrap_err()
            .outcome(),
        EffectOutcome::NotSent
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn actual_body_limits_cannot_be_raised_by_a_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"four".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    let mut output = Vec::new();
    let error = bot(&server, 3)
        .download_file(&file("a"))
        .max_bytes(100)
        .to_writer(&mut output)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ResponseTooLarge);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert!(output.is_empty());
}

#[tokio::test]
async fn chunked_bodies_are_bounded_even_without_a_content_length() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let received = stream.read(&mut request).await.unwrap();
        assert!(received > 0);
        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\nab\r\n4\r\ncdef\r\n0\r\n\r\n").await.unwrap();
    });
    let bot = Bot::builder(TOKEN)
        .api_url(format!("http://{address}/"))
        .allow_http_loopback(true)
        .max_download_bytes(3)
        .build()
        .unwrap();
    let mut output = Vec::new();
    let error = bot
        .download_file(&file("a"))
        .to_writer(&mut output)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ResponseTooLarge);
    assert!(output.len() <= 3);
    server.await.unwrap();
}

struct FailingWriter;
impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::other("private-path secret-token")))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn writer_failures_and_http_errors_have_safe_diagnostics() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/file/bot{TOKEN}/a")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1]))
        .expect(1)
        .mount(&server)
        .await;
    let bot = bot(&server, 100);
    let error = bot
        .download_file(&file("a"))
        .to_writer(&mut FailingWriter)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert!(!format!("{error:?} {error}").contains("secret-token"));
    assert!(std::error::Error::source(&error).is_none());
    Mock::given(path(format!("/file/bot{TOKEN}/b")))
        .respond_with(ResponseTemplate::new(404).set_body_string(TOKEN))
        .expect(1)
        .mount(&server)
        .await;
    let error = bot.download_file(&file("b")).bytes().await.unwrap_err();
    assert_eq!(error.http_status(), Some(404));
    assert!(!format!("{error:?} {error}").contains(TOKEN));
}

#[tokio::test]
async fn writer_backpressure_is_timed_out_and_cancellation_releases_capacity() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1; 100]))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"id":123456,"is_bot":true,"first_name":"Bot"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_in_flight(1)
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let (mut writer, _reader) = tokio::io::duplex(1);
    let error = bot
        .download_file(&file("a"))
        .to_writer(&mut writer)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let (mut writer, mut reader) = tokio::io::duplex(1);
    {
        let mut pending = Box::pin(bot.download_file(&file("a")).to_writer(&mut writer));
        let mut byte = [0];
        tokio::select! { result=&mut pending=>panic!("unexpected completion: {result:?}"), result=reader.read_exact(&mut byte)=>{ result.unwrap(); } }
    }
    bot.get_me().await.unwrap();
}

#[tokio::test]
async fn redirects_and_header_timeouts_do_not_retry_or_leak_credentials() {
    let server = MockServer::start().await;
    Mock::given(path(format!("/file/bot{TOKEN}/redirect")))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/target"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/target"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(path(format!("/file/bot{TOKEN}/slow")))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .expect(1)
        .mount(&server)
        .await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    assert_eq!(
        bot.download_file(&file("redirect"))
            .bytes()
            .await
            .unwrap_err()
            .http_status(),
        Some(302)
    );
    let error = bot.download_file(&file("slow")).bytes().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert!(!format!("{error:?} {error}").contains(TOKEN));
}
