use tg::{Bot, MessageContext, Router};

async fn start(ctx: MessageContext) -> tg::Result<()> {
    ctx.reply("Hello! Polling and command routing are running.")
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> tg::Result<()> {
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    bot.run_polling(Router::new().command("start", start)).await
}
