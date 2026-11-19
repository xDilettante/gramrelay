use crate::fixture::*;
use serde_json::json;
use std::time::Duration;
use tg::{
    ChatId, EffectOutcome,
    test_kit::{MockTransport, Step},
};

#[tokio::test]
async fn reports_unknown_after_dispatch_without_replaying() {
    let mock = MockTransport::new([Step::timeout("sendMessage")]).unwrap();
    let bot = mock
        .bot_builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let outcome = solution::solve(&bot, ChatId::from(-115), "one attempt")
        .await
        .unwrap_err();
    assert_eq!(outcome, EffectOutcome::Unknown);
    assert_eq!(
        json_body(&mock, "sendMessage"),
        json!({"chat_id":-115,"text":"one attempt"})
    );
    let mock = MockTransport::new([message("sendMessage", 88, 716)]).unwrap();
    let result = solution::solve(&mock.bot().unwrap(), ChatId::from(88), "valid success")
        .await
        .unwrap();
    assert_eq!(i64::from(result), 716);
    one(&mock, "sendMessage");
}
