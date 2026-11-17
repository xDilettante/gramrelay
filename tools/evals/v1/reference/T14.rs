pub async fn solve<W: tokio::io::AsyncWrite + Unpin + Send>(
    bot: &tg::Bot,
    file: &tg::api::types::File,
    limit: u64,
    writer: &mut W,
) -> tg::Result<u64> {
    bot.download_file(file)
        .max_bytes(limit)
        .to_writer(writer)
        .await
}
