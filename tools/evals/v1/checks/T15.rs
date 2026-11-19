use crate::fixture::*;
use tg::{
    ChatId, EffectOutcome,
    test_kit::{MockTransport, Step},
};
use tokio::io::{AsyncWriteExt, duplex};

#[tokio::test]
async fn streams_the_prefix_and_releases_the_source_after_partial_upload() {
    let mock = MockTransport::new([message("sendDocument", 600, 409)]).unwrap();
    let (mut writer, reader) = duplex(16);
    writer.write_all(b"prefix-unread").await.unwrap();
    // Keep the producer open. Buffering to EOF would hang until the runner's deadline.
    let result = solution::solve(
        &mock.bot().unwrap(),
        ChatId::from(600),
        "stream.bin".into(),
        reader,
        6,
    )
    .await
    .unwrap();
    assert_eq!(i64::from(result), 409);
    assert!(writer.write_all(b"x").await.is_err());
    let request = one(&mock, "sendDocument");
    assert!(contains(request.body(), b"filename=\"stream.bin\""));
    assert!(contains(request.body(), b"\r\n\r\nprefix\r\n"));
    assert!(!contains(request.body(), b"-unread"));

    let mock = MockTransport::new([Step::partial_upload("sendDocument", 1)]).unwrap();
    let failure = solution::solve(
        &mock.bot().unwrap(),
        ChatId::from(70),
        "file.bin".into(),
        &b"abcdef"[..],
        6,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.outcome(), EffectOutcome::Unknown);
    assert_eq!(mock.requests().unwrap().len(), 1);

    let mock = MockTransport::new([message("sendDocument", 10, 1)]).unwrap();
    let error = solution::solve(
        &mock.bot_builder().max_upload_bytes(3).build().unwrap(),
        ChatId::from(10),
        "file.bin".into(),
        &b"abcdef"[..],
        6,
    )
    .await
    .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::NotSent);
    assert!(mock.requests().unwrap().is_empty());

    let mock = MockTransport::new([message("sendDocument", 70, 1)]).unwrap();
    let failure = solution::solve(
        &mock.bot().unwrap(),
        ChatId::from(70),
        "file.bin".into(),
        &b"abc"[..],
        6,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.outcome(), EffectOutcome::Unknown);
    assert!(!mock.requests().unwrap()[0].upload_complete());
}
