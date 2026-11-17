pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    name: String,
    bytes: Vec<u8>,
) -> tg::Result<tg::MessageId> {
    let file = tg::InputFile::bytes(name, bytes);
    Ok(bot.send_document(chat, file).await?.message_id)
}
