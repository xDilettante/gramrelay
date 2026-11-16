use serde_json::json;
use tg::{ChatId, Update, api::types::MaybeInaccessibleMessage};

fn message() -> serde_json::Value {
    json!({"message_id": 1, "date": 1, "chat": {"id": -123, "type": "group"},
        "photo": [{"file_id": "photo", "file_unique_id": "unique", "width": 10, "height": 20, "future_size": 42}]})
}

#[test]
fn runtime_and_wire_models_are_the_same_complete_types() {
    let update: Update =
        serde_json::from_value(json!({"update_id": 1, "edited_message": message()})).unwrap();
    let _: &tg::api::types::Update = &update;
    let msg: &tg::Message = update.edited_message.as_ref().unwrap();
    let _: &tg::api::types::Message = msg;
    assert_eq!(msg.photo.as_ref().unwrap()[0].width, 10);
    assert_eq!(
        msg.photo.as_ref().unwrap()[0].unknown_fields()["future_size"],
        42
    );
    assert_eq!(update.known_event_count(), 1);
    assert_eq!(update.chat_id(), Some(ChatId::from(-123)));
}

#[test]
fn newly_modeled_malformed_or_ambiguous_events_are_rejected() {
    assert!(
        serde_json::from_value::<Update>(
            json!({"update_id": 1, "edited_message": {"invalid": true}})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<Update>(
            json!({"update_id": 1, "message": message(), "edited_message": message()})
        )
        .is_err()
    );
    let unknown = json!({"update_id": 1, "future_event": {"chat": {"id": -123}}});
    let decoded: Update = serde_json::from_value(unknown.clone()).unwrap();
    assert_eq!(decoded.chat_id(), None);
    assert_eq!(serde_json::to_value(decoded).unwrap(), unknown);
}

#[test]
fn inaccessible_callback_messages_keep_a_chat_without_inventing_content() {
    let update: Update = serde_json::from_value(json!({"update_id": 1, "callback_query": {
        "id": "cb", "from": {"id": 1, "is_bot": false, "first_name": "User"}, "chat_instance": "i",
        "message": {"message_id": 2, "date": 0, "chat": {"id": -123, "type": "group"}}
    }}))
    .unwrap();
    assert!(matches!(
        update
            .callback_query
            .as_ref()
            .unwrap()
            .message
            .as_ref()
            .unwrap(),
        MaybeInaccessibleMessage::InaccessibleMessage(_)
    ));
    assert_eq!(update.chat_id(), Some(ChatId::from(-123)));
}

#[cfg(feature = "bot")]
#[tokio::test]
async fn manually_constructed_ambiguous_updates_never_reach_handlers() {
    let bot = tg::Bot::new("123456:OFFLINE_TEST_TOKEN").unwrap();
    let msg: tg::Message = serde_json::from_value(message()).unwrap();
    let update = Update::new(1).message(msg.clone()).edited_message(msg);
    assert_eq!(update.known_event_count(), 2);
    assert_eq!(update.chat_id(), None);
    let router = tg::Router::new()
        .on_update(|_: tg::UpdateContext| async { panic!("ambiguous event must not dispatch") });
    let error = router.dispatch(&bot, update).await.unwrap_err();
    assert_eq!(error.field(), Some("update_kind"));
    assert_eq!(error.outcome(), tg::EffectOutcome::NotSent);
}
