pub async fn solve(
    bot: &tg::Bot,
    chat: tg::ChatId,
    documents: [(String, Vec<u8>); 2],
) -> tg::Result<Vec<tg::Message>> {
    use tg::api::types::{InputMediaDocument, SendMediaGroupMediaItem};
    let [(first_name, first), (second_name, second)] = documents;
    let media: Vec<SendMediaGroupMediaItem> = vec![
        InputMediaDocument::new("attach://first".into()).into(),
        InputMediaDocument::new("attach://second".into()).into(),
    ];
    bot.api()
        .send_media_group(chat.into(), media)
        .attach("first", tg::InputFile::bytes(first_name, first))
        .attach("second", tg::InputFile::bytes(second_name, second))
        .await
}
