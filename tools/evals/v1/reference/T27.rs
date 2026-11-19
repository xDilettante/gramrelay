pub async fn solve(
    path: &std::path::Path,
    binding: tg_mtproto::SessionBinding,
    create_key: tg_mtproto::SessionKey,
    reopen_key: tg_mtproto::SessionKey,
) -> Result<tg_mtproto::Session, tg_mtproto::SessionError> {
    use tg_mtproto::{Session, SessionOptions};
    let session =
        Session::open(path, binding.clone(), create_key, SessionOptions::default()).await?;
    session.close().await?;
    Session::open_existing(path, binding, reopen_key, SessionOptions::default()).await
}
