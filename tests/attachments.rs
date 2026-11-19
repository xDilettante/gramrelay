use serde_json::json;
use std::time::Duration;
use tg::{
    Bot, ChatId, EffectOutcome, ErrorKind, InputFile,
    api::types::{InputMediaPhoto, SendMediaGroupMediaItem},
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";
fn bot(server: &MockServer, limit: u64) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_upload_bytes(limit)
        .build()
        .unwrap()
}
fn response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": {"message_id": 1, "date": 1, "chat": {"id": 1, "type": "private"}}}))
}

#[tokio::test]
async fn named_bytes_are_lazy_and_use_typed_file_references() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response())
        .expect(1)
        .mount(&server)
        .await;
    let bot = bot(&server, 100);
    let call = bot
        .api()
        .send_photo("attach://image".into())
        .chat_id(ChatId::from(1).into())
        .caption("Report".into())
        .attach("image", InputFile::bytes("photo.bin", vec![0, 255, 1]));
    assert!(server.received_requests().await.unwrap().is_empty());
    call.await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("name=\"photo\"\r\n\r\nattach://image"));
    assert!(body.contains("name=\"image\"; filename=\"photo.bin\""));
    assert!(body.contains("name=\"caption\"\r\n\r\nReport"));
    assert!(
        requests[0]
            .body
            .windows(3)
            .any(|bytes| bytes == [0, 255, 1])
    );
}

#[tokio::test]
async fn nested_media_uploads_files_from_disk_and_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": []})))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disk.bin");
    tokio::fs::write(&path, b"disk content").await.unwrap();
    let media: Vec<SendMediaGroupMediaItem> = vec![
        InputMediaPhoto::new("attach://first".into()).into(),
        InputMediaPhoto::new("attach://second".into()).into(),
    ];
    bot(&server, 100)
        .api()
        .send_media_group(ChatId::from(1).into(), media)
        .attach("first", InputFile::path(path))
        .attach(
            "second",
            InputFile::bytes("b.bin", b"bytes content".to_vec()),
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("\"media\":\"attach://first\""));
    assert!(body.contains("disk content"));
    assert!(body.contains("bytes content"));
}

#[tokio::test]
async fn message_text_is_never_interpreted_as_an_attachment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response())
        .expect(1)
        .mount(&server)
        .await;
    bot(&server, 100)
        .api()
        .send_message("attach://ordinary_text".into())
        .chat_id(ChatId::from(1).into())
        .await
        .unwrap();
}

#[tokio::test]
async fn bad_bindings_and_combined_size_fail_without_dispatch() {
    let server = MockServer::start().await;
    let bot = bot(&server, 5);
    for name in ["bad-name", "\r\nheader", "", "photo"] {
        let error = bot
            .api()
            .send_photo(format!("attach://{name}"))
            .attach(name, InputFile::bytes("p", vec![1]))
            .await
            .unwrap_err();
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
    let missing = bot
        .api()
        .send_photo("attach://missing".into())
        .await
        .unwrap_err();
    assert_eq!(missing.outcome(), EffectOutcome::NotSent);
    let unreferenced = bot
        .api()
        .send_photo("existing_id".into())
        .attach("extra", InputFile::bytes("p", vec![1]))
        .await
        .unwrap_err();
    assert_eq!(unreferenced.outcome(), EffectOutcome::NotSent);
    let duplicate = bot
        .api()
        .send_photo("attach://same".into())
        .attach("same", InputFile::bytes("a", vec![1]))
        .attach("same", InputFile::bytes("b", vec![2]))
        .await
        .unwrap_err();
    assert_eq!(duplicate.outcome(), EffectOutcome::NotSent);
    let media = vec![
        InputMediaPhoto::new("attach://a".into()).into(),
        InputMediaPhoto::new("attach://b".into()).into(),
    ];
    let large = bot
        .api()
        .send_media_group(ChatId::from(1).into(), media)
        .attach("a", InputFile::bytes("a", vec![1; 3]))
        .attach("b", InputFile::bytes("b", vec![2; 3]))
        .await
        .unwrap_err();
    assert_eq!(large.field(), Some("upload_size"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn multipart_timeout_is_unknown_and_does_not_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response().set_delay(Duration::from_secs(5)))
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
        .send_photo("attach://p".into())
        .attach("p", InputFile::bytes("p", vec![1]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn files_open_on_await_and_local_failures_do_not_expose_paths() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response())
        .expect(1)
        .mount(&server)
        .await;
    let bot = bot(&server, 100);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private-file.bin");
    let call = bot
        .api()
        .send_photo("attach://p".into())
        .attach("p", InputFile::path(&path));
    assert!(!format!("{call:?}").contains("private-file"));
    tokio::fs::write(&path, b"created after request construction")
        .await
        .unwrap();
    call.await.unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    let missing = bot
        .api()
        .send_photo("attach://p".into())
        .attach("p", InputFile::path(&path))
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::Io);
    assert_eq!(missing.outcome(), EffectOutcome::NotSent);
    assert!(!format!("{missing:?} {missing}").contains("private-file"));
    let file_id = bot
        .api()
        .send_photo("attach://p".into())
        .attach("p", InputFile::file_id("existing_id"))
        .await
        .unwrap_err();
    assert_eq!(file_id.field(), Some("attachment_file"));
    assert_eq!(file_id.outcome(), EffectOutcome::NotSent);
}

#[tokio::test]
async fn too_many_bindings_are_rejected_before_opening_any_files() {
    let server = MockServer::start().await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_upload_bytes(100)
        .max_attachments(32)
        .build()
        .unwrap();
    let mut call = bot.api().send_photo("attach://p".into());
    for index in 0..33 {
        call = call.attach(format!("file{index}"), InputFile::path("missing-file"));
    }
    let error = call.await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Capacity);
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_default_cap_supports_fifty_rich_message_media_attachments() {
    use tg::api::types::{InputRichBlock, InputRichBlockPhoto, InputRichMessage};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let blocks: Vec<InputRichBlock> = (0..50)
        .map(|index| {
            InputRichBlockPhoto::new(InputMediaPhoto::new(format!("attach://image{index}"))).into()
        })
        .collect();
    let rich = InputRichMessage::new().blocks(blocks);
    let bot = bot(&server, 100);
    let mut call = bot
        .api()
        .send_rich_message(rich)
        .chat_id(ChatId::from(1).into());
    for index in 0..50 {
        call = call.attach(
            format!("image{index}"),
            InputFile::bytes(format!("{index}.bin"), vec![1]),
        );
    }
    call.await.unwrap();
}
