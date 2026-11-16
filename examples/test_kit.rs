use serde_json::json;
use tg::{
    ChatId,
    api::methods::SendMessage,
    test_kit::{MockResponse, MockTransport, Step},
};

#[tokio::main]
async fn main() -> tg::Result<()> {
    let mock = MockTransport::new([Step::reply(
        "sendMessage",
        MockResponse::json(json!({
            "ok":true, "result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}
        })),
    )])?;
    let bot = mock.bot()?;
    let sent = bot.send_message(ChatId::from(1), "Offline test").await?;
    let requests = mock.requests()?;
    assert_eq!(requests[0].decode::<SendMessage>()?.text, "Offline test");
    mock.assert_finished()?;
    println!(
        "In-memory test passed: message_id={}",
        i64::from(sent.message_id)
    );
    Ok(())
}
