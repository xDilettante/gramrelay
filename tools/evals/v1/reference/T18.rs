pub fn solve() -> tg::Router {
    tg::Router::new().callback("confirm", |ctx: tg::CallbackContext| async move {
        ctx.answer().await?;
        Ok(())
    })
}
