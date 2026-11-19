use tg::{Bot, Update};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: answer_callback <received_update.json>")?;
    let json = tokio::fs::read(path).await?;
    let update: Update = serde_json::from_slice(&json)?;
    let query = update
        .callback_query
        .ok_or("update contains no callback_query")?;
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    bot.answer_callback_query(query.id)
        .text("Callback received")
        .await?;
    println!("callback answered");
    Ok(())
}
