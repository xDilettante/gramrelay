pub fn solve() -> tg::Router {
    tg::Router::new().command("echo", |ctx: tg::MessageContext| async move {
        if let Some(text) = ctx.command_args() {
            ctx.reply(text).await?;
        }
        Ok(())
    })
}
