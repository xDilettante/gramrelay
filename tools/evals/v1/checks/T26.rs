use crate::fixture::*;
use tg::test_kit::MockTransport;
use tg_agent::{
    executor::{ActionCall, ChatId, ExecutorErrorKind, Request, SendDocumentArgs},
    journal::OperationState,
};

#[tokio::test]
async fn immutable_file_upload_binds_handle_chat_profile_and_original_key() {
    let mock = MockTransport::new([identity(), message("sendDocument", -67, 103)]).unwrap();
    let host = agent_fixture(&mock, "files-team", "files-user").await;
    let expires_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60_000;
    let bytes = b"fixture\x00\xff";
    let (file, receipt) = solution::solve(
        &host.executor,
        &host.policy,
        &host.actor,
        "files-team",
        ChatId::new(-67).unwrap(),
        "report-21.bin".into(),
        bytes.to_vec(),
        expires_ms,
        "key-offline",
    )
    .await
    .unwrap();
    assert_eq!(
        receipt.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(
        serde_json::to_value(&receipt).unwrap()["operation"]["operation_id"],
        "key-offline"
    );
    assert!(!file.handle().contains("report"));
    mock.assert_finished().unwrap();
    let requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method(), "sendDocument");
    assert!(contains(requests[1].body(), b"filename=\"report-21.bin\""));
    assert!(contains(requests[1].body(), bytes));
    let request = |chat, handle: &str, key: &str| {
        Request::new(
            "files-team",
            Some(key.into()),
            ActionCall::SendDocument(SendDocumentArgs {
                chat_id: ChatId::new(chat).unwrap(),
                file_handle: handle.into(),
                caption: None,
            }),
        )
        .unwrap()
    };
    let cached = host
        .executor
        .execute(
            &host.actor,
            &request(-67, file.handle(), "key-offline"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(cached).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
    let wrong_chat = host
        .executor
        .execute(&host.actor, &request(-68, file.handle(), "other"), None)
        .await;
    assert!(matches!(wrong_chat,Err(e) if e.kind() == ExecutorErrorKind::Denied));
    let substitute = host
        .executor
        .register_file(
            &host.actor,
            "files-team",
            "alternate.bin",
            b"different".to_vec(),
            expires_ms,
        )
        .await
        .unwrap();
    assert_ne!(substitute.handle(), file.handle());
    assert!(
        host.executor
            .execute(
                &host.actor,
                &request(-67, substitute.handle(), "key-offline"),
                None
            )
            .await
            .is_err()
    );
    assert_eq!(mock.requests().unwrap().len(), 2);
    host.journal.close().await.unwrap();
}
