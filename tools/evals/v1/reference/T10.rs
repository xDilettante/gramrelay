pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    text: &str,
) -> Result<tg::MessageId, tg::EffectOutcome> {
    bot.send_message(chat, text)
        .await
        .map(|value| value.message_id)
        .map_err(|error| error.outcome())
}
