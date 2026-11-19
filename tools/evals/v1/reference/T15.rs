pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    name: String,
    reader: impl tokio::io::AsyncRead + Send + 'static,
    length: u64,
) -> tg::Result<tg::MessageId> {
    let file = tg::InputFile::reader(name, reader, length);
    Ok(bot.send_document(chat, file).await?.message_id)
}
