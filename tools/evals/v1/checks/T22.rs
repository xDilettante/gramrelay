use crate::fixture::*;
use tg::test_kit::MockTransport;
use tg_agent::{
    executor::{ChatId, ExecutorErrorKind},
    journal::OperationState,
};

#[tokio::test]
async fn authorizes_only_the_supplied_profile_principal_and_chat() {
    let mock = MockTransport::new([identity(), message("sendMessage", -107, 711)]).unwrap();
    let host = agent_fixture(&mock, "bot-x", "name-x").await;
    let denied = send("bot-x", "deny-op", 563, "denied");
    let result = solution::solve(
        &host.executor,
        &host.policy,
        &host.actor,
        ChatId::new(-107).unwrap(),
        &denied,
    )
    .await;
    assert!(matches!(result, Err(error) if error.kind() == ExecutorErrorKind::Denied));
    assert!(mock.requests().unwrap().is_empty());
    let allowed = send("bot-x", "allow-op", -107, "allowed");
    let result = host
        .executor
        .execute(&host.actor, &allowed, None)
        .await
        .unwrap();
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    let other = tg_agent::executor::Principal::new("someone-else").unwrap();
    assert!(
        matches!(host.executor.execute(&other,&allowed,None).await,Err(error) if error.kind() == ExecutorErrorKind::Denied)
    );
    mock.assert_finished().unwrap();
    assert_eq!(mock.requests().unwrap().len(), 2);
    host.journal.close().await.unwrap();
}
