use serde_json::json;
use tg::{
    ChatId, InputFile,
    api::methods::SendMessage,
    test_kit::{MockResponse, MockTransport, Step},
};

#[tokio::main]
async fn main() -> tg::Result<()> {
    let message = || {
        MockResponse::json(json!({
            "ok":true, "result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}
        }))
    };
    let mock = MockTransport::new([
        Step::reply("sendMessage", message()),
        Step::reply("sendDocument", message()),
    ])?;
    let bot = mock.bot()?;
    let sent = bot.send_message(ChatId::from(1), "Offline test").await?;
    let source = &b"sample-rest"[..];
    let file = InputFile::reader("sample.txt", source, 6);
    bot.send_document(ChatId::from(1), file).await?;
    let requests = mock.requests()?;
    assert_eq!(requests[0].decode::<SendMessage>()?.text, "Offline test");
    assert!(requests[1].upload_complete());
    assert!(
        requests[1]
            .body()
            .windows(10)
            .any(|bytes| bytes == b"\r\nsample\r\n")
    );
    mock.assert_finished()?;
    println!(
        "In-memory test passed: message_id={}",
        i64::from(sent.message_id)
    );
    Ok(())
}
