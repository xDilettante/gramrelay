pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    text: &str,
    label: &str,
    data: &str,
) -> tg::Result<tg::MessageId> {
    let keyboard =
        tg::InlineKeyboardMarkup::new().row([tg::InlineKeyboardButton::callback(label, data)]);
    Ok(bot
        .send_message(chat, text)
        .reply_markup(keyboard)
        .await?
        .message_id)
}
