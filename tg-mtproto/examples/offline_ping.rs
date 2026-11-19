//! Run a scripted MTProto Ping/Pong with a temporary synthetic session.
use tg_mtproto::{
    ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl,
};
use tl::Serializable;

const PING_ID: i64 = 42;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("offline-ping", 1)?,
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await?;

    let client = test_kit::client(session, ClientOptions::default(), |_, request| async move {
        assert_eq!(request, tl::functions::Ping { ping_id: PING_ID }.to_bytes());
        Ok(tl::enums::Pong::Pong(tl::types::Pong {
            msg_id: 1,
            ping_id: PING_ID,
        })
        .to_bytes())
    })?;

    let response = client
        .invoke(&tl::functions::Ping { ping_id: PING_ID })
        .await;
    client.close().await?;
    let tl::enums::Pong::Pong(pong) = response?;
    assert_eq!(pong.ping_id, PING_ID);
    directory.close()?;
    println!("Offline MTProto Ping/Pong passed (ping_id={PING_ID}).");
    Ok(())
}
