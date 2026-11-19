use crate::fixture::*;
use serde_json::json;
use tg::{ChatId, MessageId, test_kit::MockTransport};

#[tokio::test]
async fn replies_quietly_to_the_given_parent_in_the_given_chat() {
    for (chat, parent, text, id) in [(-91, 15, "тихо", 71), (205, 64, "quiet 🦀", 79)] {
        let mock = MockTransport::new([message("sendMessage", chat, id)]).unwrap();
        let reply = solution::solve(
            &mock.bot().unwrap(),
            ChatId::from(chat),
            MessageId::from(parent),
            text,
        )
        .await
        .unwrap();
        assert_eq!(i64::from(reply), id);
        assert_eq!(
            json_body(&mock, "sendMessage"),
            json!({
                "chat_id":chat,"text":text,"disable_notification":true,"reply_parameters":{"message_id":parent}
            })
        );
    }
}
