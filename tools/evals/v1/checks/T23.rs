use crate::fixture::*;
use tg::test_kit::{MockTransport, Step};
use tg_agent::{
    executor::{Action, ChatId, ExecutorErrorKind},
    journal::OperationState,
};

#[tokio::test]
async fn keeps_the_operation_identity_across_success_and_unknown() {
    for (step, expected) in [
        (message("sendMessage", 403, 283), OperationState::Succeeded),
        (Step::timeout("sendMessage"), OperationState::Unknown),
    ] {
        let mock = MockTransport::new([identity(), step]).unwrap();
        let host = agent_fixture(&mock, "demo-rw", "operator-b").await;
        allow(
            &host,
            "demo-rw",
            ChatId::new(403).unwrap(),
            [Action::SendMessage, Action::GetOperation],
            [],
        );
        let request = send("demo-rw", "stable-request", 403, "hi there");
        let outcomes = solution::solve(&host.executor, &host.actor, &request, "stable-request")
            .await
            .unwrap();
        let receipts = outcomes.map(|value| serde_json::to_value(value).unwrap());
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(receipts[0], receipts[2]);
        assert_eq!(receipts[0]["operation"]["operation_id"], "stable-request");
        assert_eq!(
            receipts[0]["operation"]["state"],
            serde_json::to_value(expected).unwrap()
        );
        let changed = send("demo-rw", "stable-request", 403, "changed");
        let result = host.executor.execute(&host.actor, &changed, None).await;
        assert!(matches!(result,Err(error) if error.kind() == ExecutorErrorKind::Conflict));
        let requests = mock.requests().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.method() == "sendMessage")
                .count(),
            1
        );
        mock.assert_finished().unwrap();
        host.journal.close().await.unwrap();
    }
}
