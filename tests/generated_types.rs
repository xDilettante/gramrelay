use serde_json::{Value, json};
use tg::api::types::*;

#[test]
fn generated_objects_preserve_unknowns_and_use_separate_ids() {
    let message: Message = serde_json::from_value(json!({
        "message_id": 7, "date": 12, "chat": {"id": -100, "type": "supergroup", "future_chat": 1},
        "text": "Привет 🦀", "future_message": {"a": true}
    }))
    .unwrap();
    let _: tg::MessageId = message.message_id;
    let _: tg::ChatId = message.chat.id;
    assert_eq!(message.chat.unknown_fields()["future_chat"], 1);
    assert_eq!(
        message.unknown_fields()["future_message"],
        json!({"a": true})
    );
    let value = serde_json::to_value(message).unwrap();
    assert_eq!(value["text"], "Привет 🦀");
    assert!(value.get("caption").is_none());
    assert!(
        serde_json::from_value::<Message>(
            json!({"message_id": 1, "chat": {"id": 1, "type": "private"}})
        )
        .is_err()
    );
}

#[test]
fn literal_tags_and_true_flags_cannot_change() {
    let photo = InputMediaPhoto::new("attach://image".into());
    assert_eq!(
        serde_json::to_value(photo).unwrap(),
        json!({"type": "photo", "media": "attach://image"})
    );
    assert!(
        serde_json::from_value::<InputMediaPhoto>(json!({"type": "video", "media": "x"})).is_err()
    );
    assert!(serde_json::from_value::<ForceReply>(json!({"force_reply": false})).is_err());
    assert!(
        serde_json::from_value::<InaccessibleMessage>(
            json!({"chat": {"id": 1, "type": "private"}, "message_id": 1, "date": 1})
        )
        .is_err()
    );
}

#[test]
fn known_union_tags_are_strict_and_new_tags_round_trip() {
    let new = json!({"type": "future_media", "future_field": {"x": 1}});
    let media: InputMedia = serde_json::from_value(new.clone()).unwrap();
    assert!(matches!(media, InputMedia::Unknown(_)));
    assert_eq!(serde_json::to_value(media).unwrap(), new);
    assert!(serde_json::from_value::<InputMedia>(json!({"type": "photo"})).is_err());
    assert!(serde_json::from_value::<InputMedia>(json!({"media": "x"})).is_err());
    assert!(serde_json::from_value::<InputMedia>(json!({"type": 4, "media": "x"})).is_err());
    assert!(serde_json::from_value::<InputMedia>(Value::Null).is_err());
}

#[test]
fn repeated_tags_and_untagged_content_choose_complete_shapes() {
    let cached: InlineQueryResult =
        serde_json::from_value(json!({"type": "photo", "id": "1", "photo_file_id": "file"}))
            .unwrap();
    assert!(matches!(
        cached,
        InlineQueryResult::InlineQueryResultCachedPhoto(_)
    ));
    let remote: InlineQueryResult = serde_json::from_value(json!({"type": "photo", "id": "2", "photo_url": "https://example.test/p", "thumbnail_url": "https://example.test/t"})).unwrap();
    assert!(matches!(
        remote,
        InlineQueryResult::InlineQueryResultPhoto(_)
    ));
    assert!(serde_json::from_value::<InlineQueryResult>(json!({"type": "photo", "id": "3", "photo_file_id": "f", "photo_url": "p", "thumbnail_url": "t"})).is_err());
    let venue: InputMessageContent = serde_json::from_value(
        json!({"latitude": 1.0, "longitude": 2.0, "title": "Place", "address": "Street"}),
    )
    .unwrap();
    assert!(matches!(
        venue,
        InputMessageContent::InputVenueMessageContent(_)
    ));
    assert!(
        serde_json::from_value::<InputMessageContent>(
            json!({"latitude": 1.0, "longitude": 2.0, "title": "Place"})
        )
        .is_err()
    );
}

#[test]
fn inaccessible_and_recursive_rich_text_have_exact_wire_shapes() {
    assert!(std::mem::size_of::<MaybeInaccessibleMessage>() <= 64);
    assert!(std::mem::size_of::<RichText>() <= 64);
    assert!(std::mem::size_of::<Update>() <= 1024);
    let inaccessible: MaybeInaccessibleMessage = serde_json::from_value(
        json!({"chat": {"id": 1, "type": "private"}, "message_id": 3, "date": 0}),
    )
    .unwrap();
    assert!(matches!(
        inaccessible,
        MaybeInaccessibleMessage::InaccessibleMessage(_)
    ));
    let rich = json!(["a", {"type": "bold", "text": ["b", {"type": "future_text", "payload": 1}]}]);
    let text: RichText = serde_json::from_value(rich.clone()).unwrap();
    assert_eq!(serde_json::to_value(text).unwrap(), rich);
    assert!(serde_json::from_value::<RichText>(json!({"type": "bold", "text": 42})).is_err());
}
