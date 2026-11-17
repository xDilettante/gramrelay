pub async fn solve(client: &tg_mtproto::Client) -> Result<(), tg_mtproto::ClientError> {
    client.close().await
}
