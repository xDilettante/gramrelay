pub async fn solve(bot: &tg::Bot, chat: tg::ChatId, file: tg::FileId) -> tg::Result<tg::MessageId> {
    Ok(bot
        .send_document(chat, tg::InputFile::file_id(file.as_str()))
        .await?
        .message_id)
}
