pub async fn solve(
    executor: &tg_agent::executor::Executor,
    policy: &tg_agent::executor::PolicyStore,
    actor: &tg_agent::executor::Principal,
    profile: &str,
    chat: tg_agent::executor::ChatId,
    name: String,
    bytes: Vec<u8>,
    expires_ms: i64,
    key: &str,
) -> Result<
    (
        tg_agent::executor::RegisteredFile,
        tg_agent::executor::ExecutionResult,
    ),
    tg_agent::executor::ExecutorError,
> {
    use tg_agent::executor::{
        Action, ActionCall, Budget, ChatScope, Grant, Request, SendDocumentArgs,
    };
    let file = executor
        .register_file(actor, profile, name, bytes, expires_ms)
        .await?;
    policy.replace(
        actor,
        profile,
        Grant::new(0, Budget::new("file-scope")?)
            .allow(Action::SendDocument)
            .chats(ChatScope::only([chat]))
            .files([file.handle()]),
    )?;
    let request = Request::new(
        profile,
        Some(key.into()),
        ActionCall::SendDocument(SendDocumentArgs {
            chat_id: chat,
            file_handle: file.handle().into(),
            caption: None,
        }),
    )?;
    let receipt = executor.execute(actor, &request, None).await?;
    Ok((file, receipt))
}
