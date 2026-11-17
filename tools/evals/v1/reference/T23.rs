pub async fn solve(
    executor: &tg_agent::executor::Executor,
    actor: &tg_agent::executor::Principal,
    request: &tg_agent::executor::Request,
    key: &str,
) -> Result<[tg_agent::executor::ExecutionResult; 3], tg_agent::executor::ExecutorError> {
    use tg_agent::executor::{ActionCall, GetOperationArgs, Request};
    let first = executor.execute(actor, request, None).await?;
    let cached = executor.execute(actor, request, None).await?;
    let status = Request::new(
        request.profile(),
        None,
        ActionCall::GetOperation(GetOperationArgs {
            operation_id: key.into(),
        }),
    )?;
    let observed = executor.execute(actor, &status, None).await?;
    Ok([first, cached, observed])
}
