use serde_json::json;
use tg::{Bot, ChatId, EffectOutcome, ErrorKind, InputFile};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const TOKEN: &str = "123456:OFFLINE_TEST_TOKEN";

fn bot(server: &MockServer) -> Bot {
    Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}

fn document_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{
        "message_id":3,"date":1,"chat":{"id":1,"type":"private"},
        "document":{"file_id":"existing","file_unique_id":"unique","file_size":4}
    }}))
}

#[tokio::test]
async fn uploads_bytes_as_multipart_and_decodes_file_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/sendDocument")))
        .respond_with(document_response())
        .expect(1)
        .mount(&server)
        .await;
    let result = bot(&server)
        .send_document(
            ChatId::from(1),
            InputFile::bytes("report.bin", vec![0, 1, 2, 255]),
        )
        .caption("Report")
        .await
        .unwrap();
    assert_eq!(result.document.unwrap().file_id.as_str(), "existing");
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    let body = &requests[0].body;
    assert!(body.windows(4).any(|part| part == [0, 1, 2, 255]));
    let text = String::from_utf8_lossy(body);
    assert!(text.contains("filename=\"report.bin\""));
    assert!(text.contains("name=\"chat_id\"\r\n\r\n1"));
    assert!(text.contains("name=\"caption\"\r\n\r\nReport"));
}

#[tokio::test]
async fn uploads_a_file_from_disk() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(document_response())
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disk.bin");
    let content = vec![0xab; 128 * 1024];
    tokio::fs::write(&path, &content).await.unwrap();
    bot(&server)
        .send_document(ChatId::from(1), InputFile::path(&path))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .body
            .windows(content.len())
            .any(|part| part == content)
    );
    assert!(String::from_utf8_lossy(&requests[0].body).contains("filename=\"disk.bin\""));
}

#[tokio::test]
async fn missing_and_oversized_files_are_not_dispatched() {
    let server = MockServer::start().await;
    let bot = Bot::builder(TOKEN)
        .api_url(server.uri())
        .allow_http_loopback(true)
        .max_upload_bytes(3)
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let missing = bot
        .send_document(ChatId::from(1), InputFile::path(dir.path().join("missing")))
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::Io);
    assert_eq!(missing.outcome(), EffectOutcome::NotSent);
    let path = dir.path().join("oversized");
    tokio::fs::write(&path, b"four").await.unwrap();
    for input in [
        InputFile::path(&path),
        InputFile::bytes("oversized.bin", b"four".to_vec()),
    ] {
        let error = bot.send_document(ChatId::from(1), input).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn reuses_an_existing_file_id_without_uploading() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(document_response())
        .expect(1)
        .mount(&server)
        .await;
    bot(&server)
        .send_document(ChatId::from(1), InputFile::file_id("existing"))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let text = String::from_utf8_lossy(&requests[0].body);
    assert!(text.contains("name=\"document\"\r\n\r\nexisting"));
    assert!(!text.contains("filename="));
}
