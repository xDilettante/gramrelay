pub async fn solve(bot: &tg::Bot, id: tg::CallbackQueryId, notification: &str) -> tg::Result<bool> {
    bot.answer_callback_query(id).text(notification).await
}
