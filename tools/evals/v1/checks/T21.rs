use serde_json::json;
use tg_agent::journal::{Journal, JournalOptions, OperationRequest, OperationState, Preparation};

#[tokio::test]
async fn persisted_receipt_uses_original_identity_and_rejects_payload_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ops.sqlite");
    let payload = json!({"chat_id":"-198","text":"save 🦀"});
    let receipt = json!({"message_id":685});
    let result = solution::solve(
        &path,
        "work-78",
        "principal-b",
        "op-ver-13",
        &payload,
        receipt.clone(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.operation_id(), "op-ver-13");
    assert_eq!(result.profile(), "work-78");
    assert_eq!(result.action(), "send_message");
    assert_eq!(result.state(), OperationState::Succeeded);
    assert_eq!(result.result(), Some(&receipt));
    let stored = Journal::open(path, JournalOptions::default())
        .await
        .unwrap();
    let matching = OperationRequest::new(
        "work-78",
        "principal-b",
        "op-ver-13",
        "send_message",
        &payload,
    )
    .unwrap();
    assert!(matches!(
        stored.prepare(matching).await.unwrap(),
        Preparation::Existing(_)
    ));
    let changed = OperationRequest::new(
        "work-78",
        "principal-b",
        "op-ver-13",
        "send_message",
        &json!({"chat_id":"-777","text":"different"}),
    )
    .unwrap();
    assert!(stored.prepare(changed).await.is_err());
    assert!(
        stored
            .get("work-78", "principal-c", "op-ver-13")
            .await
            .unwrap()
            .is_none()
    );
    stored.close().await.unwrap();
}
