pub async fn solve<S: Send + Sync + 'static>(
    bot: &tg::Bot,
    router: tg::Router<S>,
    max_concurrency: usize,
    stop: tg::CancellationToken,
) -> tg::Result<()> {
    let options = tg::PollingOptions::default()
        .max_concurrency(max_concurrency)
        .allowed_updates(["message", "callback_query", "edited_message"]);
    bot.run_polling_until(router, options, stop).await
}
