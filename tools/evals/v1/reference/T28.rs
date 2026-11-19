pub async fn solve(
    path: &std::path::Path,
    binding: tg_mtproto::SessionBinding,
    key: tg_mtproto::SessionKey,
) -> Result<tg_mtproto::Session, tg_mtproto::SessionError> {
    tg_mtproto::Session::open_existing(path, binding, key, tg_mtproto::SessionOptions::default())
        .await
}
