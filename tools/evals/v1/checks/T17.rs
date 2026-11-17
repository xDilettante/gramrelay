use crate::fixture::*;
use serde_json::json;
use tg::{Update, test_kit::MockTransport};

#[tokio::test]
async fn command_matches_exactly_and_preserves_both_ordinary_and_scoped_reply() {
    for scoped in [false, true] {
        let reply = || message("sendMessage", -12, 160);
        let mock = MockTransport::new([reply()]).unwrap();
        let bot = mock.bot().unwrap();
        let router = solution::solve();
        let mut update = json!({"update_id":10,"message":{
          "message_id":156,"date":7,"chat":{"id":-12,"type":"supergroup"},"text":"/echoes wrong"
        }});
        if scoped {
            update["message"]["message_thread_id"] = json!(19);
            update["message"]["business_connection_id"] = json!("host-scoped-id");
        }
        let parse = |value| serde_json::from_value::<Update>(value).unwrap();
        router.dispatch(&bot, parse(update.clone())).await.unwrap();
        assert!(mock.requests().unwrap().is_empty());
        update["message"]["text"] = json!("/echo echo-🐙");
        router.dispatch(&bot, parse(update)).await.unwrap();
        let mut expected =
            json!({"chat_id":-12,"text":"echo-🐙","reply_parameters":{"message_id":156}});
        if scoped {
            expected["message_thread_id"] = json!(19);
            expected["business_connection_id"] = json!("host-scoped-id");
        }
        assert_eq!(json_body(&mock, "sendMessage"), expected);
    }
}
