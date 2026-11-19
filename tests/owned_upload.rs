#![cfg(feature = "test-kit")]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tg::{
    ChatId, InputFile,
    test_kit::{MockResponse, MockTransport, Step},
};
struct Owner {
    bytes: Vec<u8>,
    reads: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
impl AsRef<[u8]> for Owner {
    fn as_ref(&self) -> &[u8] {
        self.reads.fetch_add(1, Ordering::SeqCst);
        &self.bytes
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn owned_bytes_hold_the_owner_until_multipart_consumes_the_body_without_copying() {
    let reads = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let mock=MockTransport::new([Step::reply("sendDocument",MockResponse::json(serde_json::json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}})))]).unwrap();
    let bot = mock.bot().unwrap();
    let file = InputFile::owned_bytes(
        "owner.bin",
        Owner {
            bytes: vec![0, 255, 3],
            reads: reads.clone(),
            drops: drops.clone(),
        },
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let request = bot.send_document(ChatId::from(1), file);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(mock.requests().unwrap().is_empty());
    request.await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(
        mock.requests().unwrap()[0]
            .body()
            .windows(3)
            .any(|v| v == [0, 255, 3])
    );
    mock.assert_finished().unwrap();
}
#[tokio::test]
async fn dropping_an_unpolled_owned_upload_releases_its_owner_without_network() {
    let reads = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let mock = MockTransport::new([]).unwrap();
    let bot = mock.bot().unwrap();
    drop(bot.send_document(
        ChatId::from(1),
        InputFile::owned_bytes(
            "owner.bin",
            Owner {
                bytes: vec![1],
                reads,
                drops: drops.clone(),
            },
        ),
    ));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(mock.requests().unwrap().is_empty());
}
