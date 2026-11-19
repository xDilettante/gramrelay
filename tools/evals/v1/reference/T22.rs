pub async fn solve(
    executor: &tg_agent::executor::Executor,
    policy: &tg_agent::executor::PolicyStore,
    actor: &tg_agent::executor::Principal,
    allowed: tg_agent::executor::ChatId,
    request: &tg_agent::executor::Request,
) -> Result<tg_agent::executor::ExecutionResult, tg_agent::executor::ExecutorError> {
    use tg_agent::executor::{Action, Budget, ChatScope, Grant};
    policy.replace(
        actor,
        request.profile(),
        Grant::new(0, Budget::new("scope-fixture")?)
            .allow(Action::SendMessage)
            .chats(ChatScope::only([allowed])),
    )?;
    executor.execute(actor, request, None).await
}
