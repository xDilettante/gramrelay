use crate::fixture::*;
use serde_json::json;
use tg::{ChatId, test_kit::MockTransport};

#[tokio::test]
async fn sends_exactly_one_callback_button() {
    for (chat, text, label, data, id) in [
        (-104, "Выбор", "Ещё", "more", 15),
        (72, "Pick", "🦀", "x-105", 51),
    ] {
        let mock = MockTransport::new([message("sendMessage", chat, id)]).unwrap();
        let answer = solution::solve(&mock.bot().unwrap(), ChatId::from(chat), text, label, data)
            .await
            .unwrap();
        assert_eq!(i64::from(answer), id);
        assert_eq!(
            json_body(&mock, "sendMessage"),
            json!({
                "chat_id":chat,"text":text,"reply_markup":{"inline_keyboard":[[{"text":label,"callback_data":data}]]}
            })
        );
    }
}
