pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    message: tg::MessageId,
    text: &str,
) -> tg::Result<tg::api::types::EditMessageTextResult> {
    bot.api()
        .edit_message_text()
        .chat_id(chat.into())
        .message_id(message)
        .text(text.into())
        .await
}
