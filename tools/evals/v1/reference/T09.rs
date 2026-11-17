pub fn solve(bot: &tg::Bot, chat: tg::ChatId, text: &str) {
    drop(bot.send_message(chat, text).disable_notification(true));
}
