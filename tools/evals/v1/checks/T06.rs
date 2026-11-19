use crate::fixture::*;
use serde_json::json;
use tg::{ChatId, FileId, test_kit::MockTransport};

#[tokio::test]
async fn reuses_file_id_without_an_attachment() {
    for (chat, file, id) in [(-33, "existing-15", 980), (197, "existing-20", 601)] {
        let mock = MockTransport::new([message("sendDocument", chat, id)]).unwrap();
        let result = solution::solve(&mock.bot().unwrap(), ChatId::from(chat), FileId::from(file))
            .await
            .unwrap();
        assert_eq!(i64::from(result), id);
        let request = one(&mock, "sendDocument");
        if request.content_type() == Some("application/json") {
            let value: serde_json::Value = serde_json::from_slice(request.body()).unwrap();
            assert_eq!(value, json!({"chat_id":chat,"document":file}));
        } else {
            assert!(
                request
                    .content_type()
                    .unwrap()
                    .starts_with("multipart/form-data; boundary=")
            );
            assert!(contains(
                request.body(),
                format!("name=\"document\"\r\n\r\n{file}\r\n").as_bytes()
            ));
            assert!(contains(
                request.body(),
                format!("name=\"chat_id\"\r\n\r\n{chat}\r\n").as_bytes()
            ));
            assert!(!contains(request.body(), b"filename="));
        }
    }
}
