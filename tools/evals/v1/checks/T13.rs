use serde_json::json;

#[test]
fn distinguishes_readable_inaccessible_inline_and_malformed_updates() {
    let callback = json!({"update_id":73,"callback_query":{
        "id":"fixture/cb","from":{"id":42,"is_bot":false,"first_name":"Fixture"},
        "chat_instance":"synthetic","data":"example","message":{
            "message_id":6,"date":32,"chat":{"id":-21,"type":"private"},"text":"сообщение 🦀"
        }
    }});
    assert_eq!(
        solution::solve(&callback.to_string()).unwrap().as_deref(),
        Some("сообщение 🦀")
    );
    let mut inaccessible = callback.clone();
    inaccessible["callback_query"]["message"]["date"] = json!(0);
    assert_eq!(solution::solve(&inaccessible.to_string()).unwrap(), None);
    let mut inline = callback.clone();
    let fields = inline["callback_query"].as_object_mut().unwrap();
    fields.remove("message");
    fields.insert("inline_message_id".into(), json!("inline-fixture"));
    assert_eq!(solution::solve(&inline.to_string()).unwrap(), None);
    let mut malformed = callback;
    malformed["callback_query"]["message"]["date"] = json!("bad date");
    assert!(solution::solve(&malformed.to_string()).is_err());
}
