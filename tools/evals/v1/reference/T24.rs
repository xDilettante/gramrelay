pub fn solve(
    profile: &str,
    key: &str,
    chat: i64,
    text: &str,
) -> Result<[tg_agent::tools::ToolRequest; 2], tg_agent::executor::ExecutorError> {
    use tg_agent::executor::{ActionCall, ChatId, GetChatArgs, Request, SendMessageArgs};
    let chat_id = ChatId::new(chat)?;
    let write = Request::new(
        profile,
        Some(key.into()),
        ActionCall::SendMessage(SendMessageArgs {
            chat_id,
            text: text.into(),
            message_thread_id: None,
            disable_notification: None,
        }),
    )?;
    let read = Request::new(profile, None, ActionCall::GetChat(GetChatArgs { chat_id }))?;
    Ok([write.into(), read.into()])
}
