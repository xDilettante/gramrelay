use tg::{Bot, ChatId, InputFile};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let chat = ChatId::from(
        args.next()
            .ok_or("usage: upload_document <chat_id> <path>")?
            .parse::<i64>()?,
    );
    let path = args
        .next()
        .ok_or("usage: upload_document <chat_id> <path>")?;
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    let sent = bot.send_document(chat, InputFile::path(path)).await?;
    println!("message_id={}", i64::from(sent.id));
    Ok(())
}
