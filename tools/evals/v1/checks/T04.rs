use crate::fixture::*;
use serde_json::json;
use tg::{CallbackQueryId, test_kit::MockTransport};

#[tokio::test]
async fn acknowledges_id_without_sending_a_chat_message() {
    for (id, text) in [("Cb-71/opaque", "получено"), ("Cb-8", "🦀")] {
        let mock = MockTransport::new([reply("answerCallbackQuery", json!(true))]).unwrap();
        assert!(
            solution::solve(&mock.bot().unwrap(), CallbackQueryId::from(id), text)
                .await
                .unwrap()
        );
        assert_eq!(
            json_body(&mock, "answerCallbackQuery"),
            json!({"callback_query_id":id,"text":text})
        );
    }
}
