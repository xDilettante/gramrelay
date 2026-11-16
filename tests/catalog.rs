use tg::api::catalog;

#[test]
fn generated_inventory_matches_the_pinned_schema() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../schema/bot-api.json")).unwrap();
    let methods: Vec<_> = schema["methods"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let types: Vec<_> = schema["types"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(catalog::BOT_API_VERSION, "10.4");
    assert_eq!(catalog::SOURCE_SHA256, schema["source"]["sha256"]);
    assert_eq!(catalog::METHODS, methods);
    assert_eq!(catalog::TYPES, types);
    assert_eq!(catalog::METHODS.len(), 193);
    assert_eq!(catalog::TYPES.len(), 425);
    for name in ["sendMessage", "getUpdates", "sendWidget", "getUniqueGift"] {
        assert!(catalog::METHODS.contains(&name));
    }
}
