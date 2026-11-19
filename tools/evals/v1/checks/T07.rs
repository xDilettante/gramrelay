use crate::fixture::*;
use serde_json::json;
use tg::{ChatId, MessageId, api::types::EditMessageTextResult, test_kit::MockTransport};

#[tokio::test]
async fn edits_the_specified_chat_message_and_decodes_the_union() {
    for (chat, id, text) in [(-103, 48, "Исправлено"), (94, 77, "edited 🦀")] {
        let mock = MockTransport::new([message("editMessageText", chat, id)]).unwrap();
        let result = solution::solve(
            &mock.bot().unwrap(),
            ChatId::from(chat),
            MessageId::from(id),
            text,
        )
        .await
        .unwrap();
        let EditMessageTextResult::Message(value) = result else {
            panic!("expected message result");
        };
        assert_eq!(i64::from(value.message_id), id);
        assert_eq!(
            json_body(&mock, "editMessageText"),
            json!({"chat_id":chat,"message_id":id,"text":text})
        );
    }
}
