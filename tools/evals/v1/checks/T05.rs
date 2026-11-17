use crate::fixture::*;
use tg::{ChatId, test_kit::MockTransport};

#[tokio::test]
async fn sends_binary_bytes_and_the_requested_filename() {
    for (chat, name, bytes) in [
        (67, "one.bin", &[0, 1, 255, 2][..]),
        (-371, "two.bin", &[127, 128, 0, 59][..]),
    ] {
        let mock = MockTransport::new([message("sendDocument", chat, 613)]).unwrap();
        let result = solution::solve(
            &mock.bot().unwrap(),
            ChatId::from(chat),
            name.into(),
            bytes.to_vec(),
        )
        .await
        .unwrap();
        assert_eq!(i64::from(result), 613);
        let request = one(&mock, "sendDocument");
        assert!(
            request
                .content_type()
                .unwrap()
                .starts_with("multipart/form-data; boundary=")
        );
        assert!(contains(
            request.body(),
            format!("name=\"document\"; filename=\"{name}\"").as_bytes()
        ));
        assert!(contains(
            request.body(),
            format!("name=\"chat_id\"\r\n\r\n{chat}\r\n").as_bytes()
        ));
        let payload = [b"\r\n\r\n".as_slice(), bytes, b"\r\n"].concat();
        assert!(contains(request.body(), &payload));
    }
}
