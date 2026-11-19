use serde_json::{Value, json};
use tg_agent::tools::ToolRequest;

#[test]
fn constructs_strict_versioned_read_and_write_envelopes() {
    for (profile, key, chat, text) in [
        ("local", "write-a", -146, "первый"),
        ("archive", "write-b", 461, "🦀"),
    ] {
        let [write, read] = solution::solve(profile, key, chat, text).unwrap();
        assert!(write.is_write());
        assert_eq!(write.operation_id(), Some(key));
        assert!(!read.is_write());
        assert!(read.operation_id().is_none());
        let encode = |request| match request {
            ToolRequest::BotApi(req) => serde_json::to_value(req).unwrap(),
            _ => panic!("expected Bot API request"),
        };
        let mut encoded: Value = encode(write);
        assert_eq!(
            encoded,
            json!({"schema_version":1,"profile":profile,"operation_id":key,"call":{"tool":"send_message","arguments":{"chat_id":chat.to_string(),"text":text}}})
        );
        assert_eq!(
            encode(read),
            json!({"schema_version":1,"profile":profile,"call":{"tool":"get_chat","arguments":{"chat_id":chat.to_string()}}})
        );
        encoded["call"]["arguments"]["api_url"] = json!("https://invalid.test/");
        assert!(ToolRequest::from_json(&serde_json::to_vec(&encoded).unwrap()).is_err());
    }
    assert!(solution::solve("local", "write-a", 0, "text").is_err());
}
