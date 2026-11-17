use crate::fixture::*;
use serde_json::{Value, json};
use tg::{ChatId, test_kit::MockTransport};

#[tokio::test]
async fn ordered_document_media_references_each_actual_byte_part_once() {
    let mock = MockTransport::new([reply(
        "sendMediaGroup",
        json!([
            {"message_id":91,"date":5,"chat":{"id":-83,"type":"private"}},
            {"message_id":93,"date":5,"chat":{"id":-83,"type":"private"}}
        ]),
    )])
    .unwrap();
    let docs = [
        ("second.bin".into(), b"first content".to_vec()),
        ("first.bin".into(), b"second content".to_vec()),
    ];
    let result = solution::solve(&mock.bot().unwrap(), ChatId::from(-83), docs)
        .await
        .unwrap();
    assert_eq!(
        result
            .iter()
            .map(|m| i64::from(m.message_id))
            .collect::<Vec<_>>(),
        [91, 93]
    );
    let request = one(&mock, "sendMediaGroup");
    let boundary = format!(
        "--{}",
        request
            .content_type()
            .unwrap()
            .split_once("boundary=")
            .unwrap()
            .1
    );
    let parts = std::str::from_utf8(request.body())
        .unwrap()
        .split(&boundary);
    let mut fields = std::collections::BTreeMap::new();
    for part in parts.filter(|part| part.contains("Content-Disposition:")) {
        let (head, data) = part.split_once("\r\n\r\n").unwrap();
        let tail = head.split_once("name=\"").unwrap().1;
        let (name, options) = tail.split_once('"').unwrap();
        assert!(
            fields
                .insert(name, (options, data.strip_suffix("\r\n").unwrap()))
                .is_none()
        );
    }
    assert_eq!(fields["chat_id"].1, "-83");
    let media: Vec<Value> = serde_json::from_str(fields["media"].1).unwrap();
    assert_eq!(fields.len(), 4);
    assert_eq!(media.len(), 2);
    for (item, name, body) in [
        (&media[0], "second.bin", "first content"),
        (&media[1], "first.bin", "second content"),
    ] {
        assert_eq!(item["type"], "document");
        let part = item["media"]
            .as_str()
            .unwrap()
            .strip_prefix("attach://")
            .unwrap();
        let (options, data) = fields[part];
        assert!(options.contains(&format!("filename=\"{name}\"")));
        assert_eq!(data, body);
    }
}
