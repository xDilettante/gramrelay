pub async fn solve(bot: &tg::Bot, chat: tg::ChatId, text: &str) -> tg::Result<tg::MessageId> {
    Ok(bot.send_message(chat, text).await?.message_id)
}
