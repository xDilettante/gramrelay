use serde_json::json;

#[test]
fn extracts_unknown_values_and_preserves_nested_response_data() {
    for (key, unknown) in [
        ("future_value", json!(["next",{"field":9}])),
        ("future_tree", json!({"a":[true,null]})),
    ] {
        let mut message = json!({"message_id":6,"date":1,"chat":{"id":-22,"type":"private","future_chat":{"state":true}}});
        message[key] = unknown.clone();
        let (field, round_trip) = solution::solve(&message.to_string(), key).unwrap();
        assert_eq!(field, Some(unknown));
        assert_eq!(round_trip, message);
        assert!(
            solution::solve(&message.to_string(), "missing")
                .unwrap()
                .0
                .is_none()
        );
        message.as_object_mut().unwrap().remove("message_id");
        assert!(solution::solve(&message.to_string(), key).is_err());
    }
}
