use crate::fixture::*;
use serde_json::json;
use tg::{ChatId, test_kit::MockTransport};

#[tokio::test]
async fn sends_exact_values_and_returns_server_id() {
    for (chat, text, id) in [(-100_321, "первый 🦀", 91), (417, "第二", 605)] {
        let mock = MockTransport::new([message("sendMessage", chat, id)]).unwrap();
        let result = solution::solve(&mock.bot().unwrap(), ChatId::from(chat), text)
            .await
            .unwrap();
        assert_eq!(i64::from(result), id);
        assert_eq!(
            json_body(&mock, "sendMessage"),
            json!({"chat_id":chat,"text":text})
        );
    }
}
