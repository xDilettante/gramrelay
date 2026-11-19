use std::{
    cell::Cell,
    future::IntoFuture,
    io,
    marker::PhantomPinned,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    task::{Context, Poll},
    time::Duration,
};
use tg::{Bot, ChatId, EffectOutcome, ErrorKind, InputFile, RetryPolicy};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::Notify,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const MESSAGE: &str =
    r#"{"ok":true,"result":{"message_id":3,"date":1,"chat":{"id":1,"type":"private"}}}"#;
const DENIED: &str = r#"{"ok":false,"error_code":403,"description":"synthetic refusal"}"#;
const PRIVATE_ERROR: &str = "private synthetic reader diagnostic";

#[derive(Default)]
struct Observed {
    read: AtomicUsize,
    polls: AtomicUsize,
    dropped: AtomicBool,
    pending: Notify,
}

#[derive(Clone, Copy)]
enum End {
    Eof,
    Pending,
    Error,
}

struct Reader {
    bytes: Cell<&'static [u8]>,
    end: End,
    observed: Arc<Observed>,
    _pin: PhantomPinned,
}

impl AsyncRead for Reader {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let reader = self.as_ref().get_ref();
        reader.observed.polls.fetch_add(1, Relaxed);
        let bytes = reader.bytes.get();
        if !bytes.is_empty() {
            let len = buf.remaining().min(bytes.len());
            buf.put_slice(&bytes[..len]);
            reader.bytes.set(&bytes[len..]);
            reader.observed.read.fetch_add(len, Relaxed);
            return Poll::Ready(Ok(()));
        }
        match reader.end {
            End::Eof => Poll::Ready(Ok(())),
            End::Error => Poll::Ready(Err(io::Error::other(PRIVATE_ERROR))),
            End::Pending => {
                reader.observed.pending.notify_one();
                Poll::Pending
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.observed.dropped.store(true, Relaxed);
    }
}

fn input(name: &str, bytes: &'static [u8], end: End, length: u64) -> (InputFile, Arc<Observed>) {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<InputFile>();
    let observed = Arc::new(Observed::default());
    let reader = Reader {
        bytes: Cell::new(bytes),
        end,
        observed: observed.clone(),
        _pin: PhantomPinned,
    };
    (InputFile::reader(name, reader, length), observed)
}

fn bot(url: String) -> tg::BotBuilder {
    Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(url)
        .allow_http_loopback(true)
        .timeout(Duration::from_secs(5))
}

async fn mock_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(MESSAGE, "application/json"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn real_multipart_uploads_exact_prefix_and_never_polls_an_empty_part() {
    let server = mock_server().await;
    let bot = bot(server.uri()).build().unwrap();
    let (file, observed) = input("report.bin", b"exact-prefix-secret-suffix", End::Error, 12);
    let request = bot.send_document(ChatId::from(1), file);
    assert_eq!(observed.polls.load(Relaxed), 0);
    assert_eq!(i64::from(request.await.unwrap().message_id), 3);
    assert_eq!(observed.read.load(Relaxed), 12);
    assert!(observed.dropped.load(Relaxed));
    let (empty, observed) = input("empty.bin", b"", End::Error, 0);
    bot.send_document(ChatId::from(1), empty).await.unwrap();
    assert_eq!(observed.polls.load(Relaxed), 0);
    assert!(observed.dropped.load(Relaxed));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("filename=\"report.bin\""));
    assert!(body.contains("\r\n\r\nexact-prefix\r\n"));
    assert!(!body.contains("secret-suffix"));
}

#[tokio::test]
async fn invalid_uploads_fail_without_polling_and_lazy_drop_releases_reader() {
    let server = mock_server().await;
    let bot = bot(server.uri()).max_upload_bytes(3).build().unwrap();
    for (name, length, field) in [
        ("bad/name", 1, "file_name"),
        ("large.bin", 4, "upload_size"),
    ] {
        let (file, observed) = input(name, b"", End::Error, length);
        let error = bot.send_document(ChatId::from(1), file).await.unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
        assert_eq!(error.field(), Some(field));
        assert_eq!(observed.polls.load(Relaxed), 0);
        assert!(observed.dropped.load(Relaxed));
    }
    let (file, observed) = input("lazy.bin", b"one", End::Error, 3);
    drop(bot.send_document(ChatId::from(1), file));
    assert_eq!(observed.polls.load(Relaxed), 0);
    assert!(observed.dropped.load(Relaxed));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn reader_shortfall_and_io_failure_are_safe_unknown_even_at_u64_max() {
    let server = mock_server().await;
    let bot = bot(server.uri())
        .max_upload_bytes(u64::MAX)
        .build()
        .unwrap();
    for (end, length) in [(End::Eof, u64::MAX), (End::Error, 6)] {
        let (file, observed) = input("short.bin", b"part", end, length);
        let error = bot.send_document(ChatId::from(1), file).await.unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::Unknown);
        assert!(!format!("{error:?} {error}").contains(PRIVATE_ERROR));
        assert!(std::error::Error::source(&error).is_none());
        assert_eq!(observed.read.load(Relaxed), 4);
        assert!(observed.dropped.load(Relaxed));
    }
}

fn generated_document(
    bot: &Bot,
    file: InputFile,
) -> tg::api::Call<'_, tg::api::methods::SendDocument> {
    bot.api()
        .send_document("attach://file".into())
        .chat_id(ChatId::from(1).into())
        .attach("file", file)
}

#[tokio::test]
async fn named_reader_attachments_share_validation_and_refuse_retry_before_polling() {
    let server = mock_server().await;
    let bot = bot(server.uri()).build().unwrap();
    let (file, observed) = input("named.bin", b"named-bytes", End::Error, 11);
    generated_document(&bot, file).await.unwrap();
    assert_eq!(observed.read.load(Relaxed), 11);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("name=\"file\"; filename=\"named.bin\""));
    assert!(body.contains("\r\n\r\nattach://file\r\n"));
    let (file, observed) = input("once.bin", b"", End::Error, 1);
    let error = generated_document(&bot, file)
        .retry_policy(RetryPolicy::default().retry_rejected_writes(true))
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert_eq!(error.field(), Some("retry_attachments"));
    assert_eq!(observed.polls.load(Relaxed), 0);
    assert!(observed.dropped.load(Relaxed));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn nested_reader_attachments_share_the_total_budget_before_dispatch() {
    use tg::api::types::{InputMediaDocument, SendMediaGroupMediaItem};

    let server = mock_server().await;
    let bot = bot(server.uri()).max_upload_bytes(7).build().unwrap();
    let (first, first_observed) = input("first.bin", b"part", End::Error, 4);
    let (second, second_observed) = input("second.bin", b"part", End::Error, 4);
    let media: Vec<SendMediaGroupMediaItem> = vec![
        InputMediaDocument::new("attach://first".into()).into(),
        InputMediaDocument::new("attach://second".into()).into(),
    ];
    let error = bot
        .api()
        .send_media_group(ChatId::from(1).into(), media)
        .attach("first", first)
        .attach("second", second)
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert_eq!(error.field(), Some("upload_size"));
    for observed in [first_observed, second_observed] {
        assert_eq!(observed.polls.load(Relaxed), 0);
        assert!(observed.dropped.load(Relaxed));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(feature = "test-kit")]
#[tokio::test]
async fn mock_upload_deadline_drops_reader_and_records_one_incomplete_attempt() {
    use tg::test_kit::{MockResponse, MockTransport, Step};

    let mock = MockTransport::new([Step::reply(
        "sendDocument",
        MockResponse::json(serde_json::from_str::<serde_json::Value>(MESSAGE).unwrap()),
    )])
    .unwrap();
    let bot = mock
        .bot_builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let (file, observed) = input("pending.bin", b"part", End::Pending, 8);
    let error = bot.send_document(ChatId::from(1), file).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].upload_complete());
    assert_eq!(observed.read.load(Relaxed), 4);
    assert!(observed.dropped.load(Relaxed));
    mock.assert_finished().unwrap();
}

async fn read_request_head(listener: &TcpListener) -> TcpStream {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8 * 1024);
        head.push(stream.read_u8().await.unwrap());
    }
    stream
}

async fn early_response(
    listener: &TcpListener,
    observed: &Observed,
    status: &str,
    body: &str,
) -> TcpStream {
    let mut stream = read_request_head(listener).await;
    observed.pending.notified().await;
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
    // Keep the socket and unread upload open until the client reports its result.
    stream
}

#[tokio::test]
async fn real_early_success_is_unknown_and_releases_the_pending_reader() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = bot(format!("http://{}", listener.local_addr().unwrap()))
        .build()
        .unwrap();
    let (file, observed) = input("pending.bin", b"part", End::Pending, 8);
    let (_socket, result) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            early_response(&listener, &observed, "200 OK", MESSAGE),
            generated_document(&bot, file).into_future(),
        )
    })
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert_eq!(error.kind(), ErrorKind::Io);
    assert!(observed.dropped.load(Relaxed));
}

#[tokio::test]
async fn real_early_refusal_keeps_rejected_and_releases_reader() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = bot(format!("http://{}", listener.local_addr().unwrap()))
        .build()
        .unwrap();
    let (file, observed) = input("pending.bin", b"", End::Pending, 4);
    let (_socket, result) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            early_response(&listener, &observed, "403 Forbidden", DENIED),
            bot.send_document(ChatId::from(1), file).into_future(),
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().outcome(), EffectOutcome::Rejected);
    assert!(observed.dropped.load(Relaxed));
}

#[tokio::test]
async fn short_reader_racing_a_real_success_response_cannot_confirm_upload() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = bot(format!("http://{}", listener.local_addr().unwrap()))
        .build()
        .unwrap();
    let (mut writer, reader) = tokio::io::duplex(16);
    writer.write_all(b"partial-fixture").await.unwrap();
    let respond = async move {
        let mut socket = read_request_head(&listener).await;
        let mut body = Vec::new();
        while !body.ends_with(b"partial-fixture") {
            assert!(body.len() < 8 * 1024);
            body.push(socket.read_u8().await.unwrap());
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{MESSAGE}",
            MESSAGE.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        // Wake the body with a real short EOF as the early response is readable.
        drop(writer);
        socket
    };
    let (_socket, result) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            respond,
            bot.send_document(ChatId::from(1), InputFile::reader("short.bin", reader, 20))
                .into_future(),
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().outcome(), EffectOutcome::Unknown);
}

#[tokio::test]
async fn dropping_a_pending_upload_releases_reader_and_admission() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bot = bot(format!("http://{}", listener.local_addr().unwrap()))
        .max_in_flight(1)
        .build()
        .unwrap();
    let (file, observed) = input("pending.bin", b"", End::Pending, 4);
    let mut upload = bot.send_document(ChatId::from(1), file).into_future();
    let _socket = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::select! {
            _ = &mut upload => panic!("request should remain pending"),
            socket = async {
                let socket = read_request_head(&listener).await;
                observed.pending.notified().await;
                socket
            } => socket,
        }
    })
    .await
    .unwrap();
    assert_eq!(bot.get_me().await.unwrap_err().kind(), ErrorKind::Capacity);
    drop(upload);
    assert!(observed.dropped.load(Relaxed));
    let (second, second_observed) = input("second.bin", b"", End::Pending, 4);
    let mut second = bot.send_document(ChatId::from(1), second).into_future();
    tokio::time::timeout(Duration::from_secs(8), async {
        tokio::select! {
            _ = &mut second => panic!("second request should reach reader"),
            _ = second_observed.pending.notified() => (),
        }
    })
    .await
    .unwrap();
    drop(second);
    assert!(second_observed.dropped.load(Relaxed));
}
