use serde_json::json;
use tg::{
    ChatId,
    test_kit::{MockResponse, MockTransport, Step},
};
use tg_agent::journal::{Completion, Journal, JournalOptions, OperationRequest, Preparation};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let journal = Journal::open(
        directory.path().join("operations.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let mock = MockTransport::new([Step::reply(
        "sendMessage",
        MockResponse::json(json!({
            "ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}
        })),
    )])?;
    let bot = mock.bot()?;
    let params = json!({"chat_id":1,"text":"Offline durable example"});
    let request = OperationRequest::new(
        "demo",
        "trusted-host",
        "fixed-operation-id",
        "send_message",
        &params,
    )?;
    for _ in 0..2 {
        let snapshot = match journal.prepare(request.clone()).await? {
            Preparation::Ready(lease) => {
                let flight = journal.begin(lease).await?;
                let completion = match bot
                    .send_message(ChatId::from(1), "Offline durable example")
                    .await
                {
                    Ok(message) => {
                        Completion::succeeded(json!({"message_id":i64::from(message.message_id)}))
                    }
                    Err(error) => Completion::from_error(&error),
                };
                // Failure here may follow an external effect: query the same key; never invent a retry key.
                journal.complete(flight, completion).await?
            }
            Preparation::Existing(snapshot) => snapshot,
        };
        println!("operation state: {:?}", snapshot.state());
    }
    assert_eq!(mock.requests()?.len(), 1);
    mock.assert_finished()?;
    journal.close().await?;
    println!("Two invocations, one in-memory Telegram request; no sockets or credentials.");
    Ok(())
}
