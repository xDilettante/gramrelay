use crate::fixture::*;
use tg::{ChatId, test_kit::MockTransport};

#[tokio::test]
async fn dropping_builder_does_not_consume_mock_step() {
    let mock = MockTransport::new([message("sendMessage", -64, 72)]).unwrap();
    let bot = mock.bot().unwrap();
    solution::solve(&bot, ChatId::from(-64), "discard");
    tokio::task::yield_now().await;
    assert!(mock.requests().unwrap().is_empty());
    assert_eq!(
        i64::from(
            bot.send_message(ChatId::from(-64), "after")
                .await
                .unwrap()
                .message_id
        ),
        72
    );
    assert_eq!(json_body(&mock, "sendMessage")["text"], "after");
}
