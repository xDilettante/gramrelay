pub async fn solve(
    client: &tg_mtproto::Client,
    ping_id: i64,
) -> Result<tg_mtproto::tl::enums::Pong, tg_mtproto::ClientError> {
    client
        .invoke(&tg_mtproto::tl::functions::Ping { ping_id })
        .await
}
