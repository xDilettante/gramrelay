#![allow(dead_code)]

use serde_json::{Value, json};
use tg::test_kit::{MockResponse, MockTransport, RecordedRequest, Step};

pub fn reply(method: &'static str, value: Value) -> Step {
    Step::reply(
        method,
        MockResponse::json(json!({"ok":true,"result":value})),
    )
}

pub fn message(method: &'static str, chat: i64, id: i64) -> Step {
    reply(
        method,
        json!({"message_id":id,"date":51,"chat":{"id":chat,"type":"private"}}),
    )
}

pub fn one(mock: &MockTransport, method: &str) -> RecordedRequest {
    mock.assert_finished().unwrap();
    let mut requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.method(), method);
    assert_eq!(request.http_method(), "POST");
    assert!(request.upload_complete());
    request
}

pub fn json_body(mock: &MockTransport, method: &str) -> Value {
    serde_json::from_slice(one(mock, method).body()).unwrap()
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|slice| slice == needle)
}
