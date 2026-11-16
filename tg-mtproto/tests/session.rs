use tg_mtproto::{Session, SessionBinding, SessionErrorKind, SessionKey, SessionOptions};

async fn open(path: &std::path::Path) -> Result<Session, tg_mtproto::SessionError> {
    Session::open(
        path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
}

fn binding(profile: &str) -> SessionBinding {
    SessionBinding::new(profile, 1).unwrap()
}

#[tokio::test]
async fn an_extra_trigger_cannot_hide_behind_the_sqlite_internal_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    open(&path).await.unwrap().close().await.unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER sqliteXlease AFTER UPDATE ON mtproto_session BEGIN UPDATE mtproto_session SET owner=NULL,lease_until_ms=NULL WHERE id=1; END;").unwrap();
    let ciphertext: Vec<u8> = connection
        .query_row("SELECT ciphertext FROM mtproto_session", [], |row| {
            row.get(0)
        })
        .unwrap();
    let result = open(&path).await;
    if let Ok(session) = &result {
        session.close().await.unwrap();
    }
    assert_eq!(
        result.unwrap_err().kind(),
        SessionErrorKind::UnsupportedSchema
    );
    assert_eq!(
        ciphertext,
        connection
            .query_row("SELECT ciphertext FROM mtproto_session", [], |row| row
                .get::<_, Vec<u8>>(0))
            .unwrap()
    );
}

#[tokio::test]
async fn reopen_authenticates_the_original_key_and_host_context_without_resetting_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    let session = Session::open(
        &path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    assert!(session.remembered_account().unwrap().is_none());
    session.close().await.unwrap();
    for (profile, key) in [("demo", [8; 32]), ("other", [7; 32])] {
        let error = Session::open(
            &path,
            binding(profile),
            SessionKey::from_bytes(key),
            SessionOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), SessionErrorKind::Authentication);
        assert!(!format!("{error:?} {error}").contains("session.sqlite"));
        assert!(std::error::Error::source(&error).is_none());
    }
    let session = Session::open(
        &path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn one_owner_and_close_all_clones_are_enforced_before_any_network_backend_exists() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    let session = Session::open(
        &path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    let error = Session::open(
        &path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), SessionErrorKind::Owned);
    let clone = session.clone();
    session.close().await.unwrap();
    assert_eq!(
        clone.remembered_account().unwrap_err().kind(),
        SessionErrorKind::Closed
    );
    let reopened = Session::open(
        &path,
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn every_authenticated_header_and_ciphertext_rejects_tampering_without_reset() {
    for column in ["storage_id", "revision", "nonce", "ciphertext"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.sqlite");
        open(&path).await.unwrap().close().await.unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let sql = if column == "revision" {
            "UPDATE mtproto_session SET revision=revision+1".to_owned()
        } else {
            let old: Vec<u8> = connection
                .query_row(
                    &format!("SELECT {column} FROM mtproto_session"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut changed = old;
            changed[0] ^= 1;
            connection
                .execute(
                    &format!("UPDATE mtproto_session SET {column}=?1"),
                    [changed],
                )
                .unwrap();
            String::new()
        };
        if !sql.is_empty() {
            connection.execute(&sql, []).unwrap();
        }
        let before: Vec<u8> = connection
            .query_row("SELECT ciphertext FROM mtproto_session", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            open(&path).await.unwrap_err().kind(),
            SessionErrorKind::Authentication,
            "{column}"
        );
        let after: Vec<u8> = connection
            .query_row("SELECT ciphertext FROM mtproto_session", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, after);
        let owner: Option<Vec<u8>> = connection
            .query_row("SELECT owner FROM mtproto_session", [], |row| row.get(0))
            .unwrap();
        assert!(owner.is_none());
    }
}

#[tokio::test]
async fn unknown_schema_is_preserved_and_a_different_application_cannot_decrypt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    open(&path).await.unwrap().close().await.unwrap();
    let error = Session::open(
        &path,
        SessionBinding::new("demo", 2).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), SessionErrorKind::Authentication);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    assert_eq!(
        open(&path).await.unwrap_err().kind(),
        SessionErrorKind::UnsupportedSchema
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    connection.pragma_update(None, "user_version", 1).unwrap();
    open(&path).await.unwrap().close().await.unwrap();
}

#[tokio::test]
async fn dropping_the_last_handle_releases_the_owner_without_a_runtime_block() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    drop(open(&path).await.unwrap());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match open(&path).await {
            Ok(session) => {
                session.close().await.unwrap();
                break;
            }
            Err(error)
                if error.kind() == SessionErrorKind::Owned && std::time::Instant::now() < until =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await
            }
            Err(error) => panic!("owner was not released: {error}"),
        }
    }
}

#[tokio::test]
#[ignore = "subprocess fixture: invoked only with synthetic test paths"]
async fn lease_holder() {
    let path = std::env::var_os("TG_MTPROTO_TEST_SESSION").unwrap();
    let marker = std::env::var_os("TG_MTPROTO_TEST_READY").unwrap();
    let session = Session::open(
        std::path::PathBuf::from(path),
        binding("demo"),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default().lease(std::time::Duration::from_secs(2)),
    )
    .await
    .unwrap();
    std::fs::write(marker, b"ready").unwrap();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        session.remembered_account().unwrap();
    }
}

#[tokio::test]
async fn a_killed_process_keeps_ownership_until_lease_expiry_then_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite");
    let marker = directory.path().join("ready");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "lease_holder"])
        .env("TG_MTPROTO_TEST_SESSION", &path)
        .env("TG_MTPROTO_TEST_READY", &marker)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() {
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture did not acquire the synthetic session");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        open(&path).await.unwrap_err().kind(),
        SessionErrorKind::Owned
    );
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        open(&path).await.unwrap_err().kind(),
        SessionErrorKind::Owned
    );
    let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        match open(&path).await {
            Ok(session) => {
                session.close().await.unwrap();
                break;
            }
            Err(error)
                if error.kind() == SessionErrorKind::Owned && std::time::Instant::now() < until =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await
            }
            Err(error) => panic!("expired owner could not be recovered: {error}"),
        }
    }
}
