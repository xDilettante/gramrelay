pub async fn solve(bot: &tg::Bot) -> tg::Result<tg::User> {
    use std::time::Duration;
    let policy = tg::RetryPolicy::default()
        .max_attempts(2)
        .max_elapsed(Duration::from_secs(1))
        .initial_delay(Duration::from_millis(1))
        .jitter(false);
    bot.api().get_me().retry_policy(policy).await
}
