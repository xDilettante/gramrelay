pub async fn solve<S: Send + Sync + 'static>(
    bot: &tg::Bot,
    router: tg::Router<S>,
    secret: tg::webhook::WebhookSecret,
    listener: tokio::net::TcpListener,
    max_body_bytes: usize,
    stop: tg::CancellationToken,
) -> tg::Result<()> {
    let options = tg::webhook::WebhookOptions::default().max_body_bytes(max_body_bytes);
    bot.webhook(router, secret)
        .options(options)
        .serve_until(listener, stop)
        .await
}
