pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    parent: tg::MessageId,
    text: &str,
) -> tg::Result<tg::MessageId> {
    let reply = tg::api::types::ReplyParameters::new().message_id(parent);
    Ok(bot
        .send_message(chat, text)
        .disable_notification(true)
        .reply_parameters(reply)
        .await?
        .message_id)
}
