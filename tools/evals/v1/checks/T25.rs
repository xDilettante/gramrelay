#[test]
fn preserves_bot_catalog_in_extended_discovery() {
    let (bot, extended) = solution::solve();
    assert_eq!(bot.schema_version, 1);
    assert_eq!(extended.schema_version, 2);
    assert_eq!(bot.tools.len(), 8);
    assert_eq!(extended.tools.len(), 16);
    let bot_names: std::collections::BTreeSet<_> =
        bot.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        bot_names,
        std::collections::BTreeSet::from([
            "get_me",
            "get_chat",
            "read_updates",
            "send_message",
            "edit_message_text",
            "answer_callback_query",
            "get_operation",
            "send_document"
        ])
    );
    assert_eq!(
        serde_json::to_value(&bot.tools).unwrap(),
        serde_json::to_value(&extended.tools[..8]).unwrap()
    );
    let mut names = std::collections::BTreeSet::new();
    for tool in extended.tools.iter().skip(8) {
        assert!(tool.name.starts_with("mtproto_"));
        assert!(names.insert(&tool.name));
        assert!(!tool.description.is_empty());
        assert!(tool.input_schema.is_object());
        assert!(tool.output_schema.is_object());
    }
}
