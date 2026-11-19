use crate::fixture::*;
use serde_json::json;
use tg::test_kit::MockTransport;

#[tokio::test]
async fn exact_callback_data_is_acknowledged_even_without_a_message() {
    let mock = MockTransport::new([reply("answerCallbackQuery", json!(true))]).unwrap();
    let bot = mock.bot().unwrap();
    let router = solution::solve();
    let mut update = json!({"update_id":7,"callback_query":{
        "id":"opaque-callback-id","from":{"id":61,"is_bot":false,"first_name":"Fixture"},
        "chat_instance":"fixture","inline_message_id":"inline-id","data":"confirm-other"
    }});
    router
        .dispatch(&bot, serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    assert!(mock.requests().unwrap().is_empty());
    update["callback_query"]["data"] = json!("confirm");
    router
        .dispatch(&bot, serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    let body = json_body(&mock, "answerCallbackQuery");
    assert_eq!(body["callback_query_id"], "opaque-callback-id");
    assert!(body.get("chat_id").is_none());
}
