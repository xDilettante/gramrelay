use tg::{Bot, ChatId};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let chat = ChatId::from(
        args.next()
            .ok_or("usage: send_message <chat_id> <text>")?
            .parse::<i64>()?,
    );
    let text = args.next().ok_or("usage: send_message <chat_id> <text>")?;
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    let sent = bot.send_message(chat, text).await?;
    println!("message_id={}", i64::from(sent.message_id));
    Ok(())
}
