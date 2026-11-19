use super::*;
use crate::DownloadOptions;

fn document_lookup(size: u64) -> Vec<u8> {
    let mut raw = message(99, 42, "private source message", false);
    if let tl::enums::Message::Message(raw) = &mut raw {
        raw.media = Some(document_media(size));
    }
    tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
        messages: vec![raw],
        topics: vec![],
        chats: vec![],
        users: vec![],
    })
    .to_bytes()
}
fn lookup_message(raw: tl::enums::Message) -> Vec<u8> {
    tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
        messages: vec![raw],
        topics: vec![],
        chats: vec![],
        users: vec![],
    })
    .to_bytes()
}
async fn source(client: &Client) -> (crate::Account, crate::RemoteDocument) {
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let document = account.document(&peer, 99).await.unwrap().unwrap();
    (account, document)
}
fn rpc(code: i32, name: &str, value: Option<u32>) -> grammers_mtsender::InvocationError {
    grammers_mtsender::InvocationError::Rpc(grammers_mtsender::RpcError {
        code,
        name: name.into(),
        value,
        caused_by: None,
    })
}
fn file_part(bytes: Vec<u8>) -> Vec<u8> {
    let mut reply = tl::types::upload::File::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    reply.extend_from_slice(&tl::types::storage::FileUnknown::CONSTRUCTOR_ID.to_le_bytes());
    reply.extend_from_slice(&123i32.to_le_bytes());
    reply.extend_from_slice(&bytes.to_bytes());
    reply
}

#[tokio::test]
async fn document_lookup_mints_private_capability_and_downloads_exact_bounded_parts() {
    use sha2::Digest;
    let (_directory, stored) = session().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
    let client = fixture(
        stored,
        ClientOptions::default().max_response_bytes(8192),
        move |dc, bytes| {
            let calls = calls.clone();
            async move {
                assert_eq!(dc, 2);
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                {
                    let request =
                        tl::functions::messages::GetMessages::from_bytes(&bytes[4..]).unwrap();
                    assert!(
                        matches!(request.id.as_slice(), [tl::enums::InputMessage::Id(id)] if id.id == 99)
                    );
                    return Ok(document_lookup(6000));
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::upload::GetFile::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request = tl::functions::upload::GetFile::from_bytes(&bytes[4..]).unwrap();
                assert!(!request.cdn_supported);
                assert_eq!(request.limit, 4096);
                assert!(
                    matches!(request.location, tl::enums::InputFileLocation::InputDocumentFileLocation(location) if location.id == 123 && location.file_reference == b"private file ref")
                );
                let index = calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.offset, if index == 0 { 0 } else { 4096 });
                Ok(file_part(vec![7; if index == 0 { 4096 } else { 1904 }]))
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let document = account.document(&peer, 99).await.unwrap().unwrap();
    assert_eq!(document.size(), 6000);
    assert!(!format!("{document:?}").contains("private"));
    let wanted: [u8; 32] = sha2::Sha256::digest(vec![7; 6000]).into();
    let result = account
        .download_document(
            &document,
            DownloadOptions::default().expected_sha256(wanted),
        )
        .await
        .unwrap();
    assert_eq!(result.bytes(), vec![7; 6000]);
    assert_eq!(result.receipt().sha256(), wanted);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(!format!("{result:?}").contains("private"));
    client.close().await.unwrap();
}

#[tokio::test]
async fn document_lookup_rejects_foreign_or_malformed_source_and_preserves_absence() {
    for case in 0..8 {
        let (_directory, stored) = session().await;
        let client = fixture(
            stored,
            ClientOptions::default(),
            move |_, bytes| async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                );
                let mut raw = message(
                    if case == 0 { 100 } else { 99 },
                    if case == 1 { 43 } else { 42 },
                    "private",
                    false,
                );
                if let tl::enums::Message::Message(raw) = &mut raw {
                    if case != 7 {
                        raw.media = Some(document_media(10));
                    }
                    if let Some(tl::enums::MessageMedia::Document(media)) = &mut raw.media
                        && let Some(tl::enums::Document::Document(document)) = &mut media.document
                    {
                        match case {
                            2 => document.file_reference.clear(),
                            3 => document.dc_id = 0,
                            4 => document.size = -1,
                            5 => document.id = 0,
                            6 => document.attributes.push(document.attributes[0].clone()),
                            _ => {}
                        }
                    }
                }
                Ok(lookup_message(raw))
            },
        );
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        let result = account.document(&peer, 99).await;
        if case == 7 {
            assert!(result.unwrap().is_none());
        } else {
            assert_eq!(
                result.unwrap_err().outcome(),
                Outcome::Unknown,
                "case {case}"
            );
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn download_failures_never_accept_partial_data_or_replay_ambiguous_reads() {
    for case in 0..7 {
        let (_directory, stored) = session().await;
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = reads.clone();
        let client = fixture(
            stored,
            ClientOptions::default().max_response_bytes(8192),
            move |_, bytes| {
                let calls = calls.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    if bytes[..4]
                        == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                    {
                        return Ok(document_lookup(10));
                    }
                    assert_eq!(
                        bytes[..4],
                        tl::functions::upload::GetFile::CONSTRUCTOR_ID.to_le_bytes()
                    );
                    calls.fetch_add(1, Ordering::SeqCst);
                    match case {
                        0 => Ok(file_part(vec![9; 9])),
                        1 => Ok(file_part(vec![9; 11])),
                        2 => Err(rpc(400, "FILE_REFERENCE_EXPIRED", None)),
                        3 => Err(rpc(420, "FLOOD_WAIT", Some(3))),
                        4 => Err(rpc(500, "private reflected error", None)),
                        5 => Ok(tl::enums::upload::File::CdnRedirect(
                            tl::types::upload::FileCdnRedirect {
                                dc_id: 3,
                                file_token: b"private CDN token".to_vec(),
                                encryption_key: vec![1; 32],
                                encryption_iv: vec![2; 16],
                                file_hashes: vec![],
                            },
                        )
                        .to_bytes()),
                        _ => Ok(file_part(vec![9; 10])),
                    }
                }
            },
        );
        let (account, document) = source(&client).await;
        let options = if case == 6 {
            DownloadOptions::default().expected_sha256([0; 32])
        } else {
            DownloadOptions::default()
        };
        let failure = account
            .download_document(&document, options)
            .await
            .unwrap_err();
        assert_eq!(
            failure.kind(),
            match case {
                0 | 1 | 6 => ClientErrorKind::Integrity,
                2 => ClientErrorKind::FileReferenceExpired,
                3 => ClientErrorKind::FloodWait,
                4 => ClientErrorKind::Rpc,
                _ => ClientErrorKind::UnsupportedOperation,
            }
        );
        assert_eq!(
            failure.outcome(),
            if case == 2 || case == 3 {
                Outcome::Rejected
            } else {
                Outcome::Unknown
            }
        );
        assert!(!format!("{failure:?}").contains("private"));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn download_orders_concurrent_parts_and_validates_zero_length_by_one_probe() {
    for size in [0, 9000] {
        let (_directory, stored) = session().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(if size == 0 { 1 } else { 3 }));
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = reads.clone();
        let client = fixture(
            stored,
            ClientOptions::default().max_response_bytes(8192),
            move |_, bytes| {
                let barrier = barrier.clone();
                let calls = calls.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    if bytes[..4]
                        == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                    {
                        return Ok(document_lookup(size));
                    }
                    let request = tl::functions::upload::GetFile::from_bytes(&bytes[4..]).unwrap();
                    calls.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    if request.offset == 0 {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    let end = (request.offset as u64 + request.limit as u64).min(size);
                    Ok(file_part(
                        (request.offset as u64..end)
                            .map(|offset| (offset % 251) as u8)
                            .collect(),
                    ))
                }
            },
        );
        let (account, document) = source(&client).await;
        let result = account
            .download_document(&document, DownloadOptions::default().parallelism(3))
            .await
            .unwrap();
        assert_eq!(
            result.bytes(),
            (0..size)
                .map(|offset| (offset % 251) as u8)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.receipt().size(), size);
        assert_eq!(reads.load(Ordering::SeqCst), if size == 0 { 1 } else { 3 });
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn download_file_publishes_only_validated_content_and_never_overwrites() {
    let (directory, stored) = session().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let calls = reads.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let calls = calls.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(document_lookup(10));
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(file_part(vec![7; 10]))
        }
    });
    let (account, document) = source(&client).await;
    let output = directory.path().join("private-destination.bin");
    let error = account
        .download_document_to(
            &document,
            &output,
            DownloadOptions::default().expected_sha256([0; 32]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::Integrity);
    assert!(!output.exists());
    assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tg-download-")
    }));
    let receipt = account
        .download_document_to(&document, &output, DownloadOptions::default())
        .await
        .unwrap();
    assert_eq!(receipt.size(), 10);
    assert_eq!(std::fs::read(&output).unwrap(), vec![7; 10]);
    let before = reads.load(Ordering::SeqCst);
    let failure = account
        .download_document_to(&document, &output, DownloadOptions::default())
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::File);
    assert_eq!(reads.load(Ordering::SeqCst), before);
    assert_eq!(std::fs::read(&output).unwrap(), vec![7; 10]);
    assert!(!format!("{failure:?}").contains("private"));
    client.close().await.unwrap();
}

#[tokio::test]
async fn destination_created_during_download_is_preserved_and_temporary_removed() {
    let (directory, stored) = session().await;
    let output = directory.path().join("racing-destination.bin");
    let race = output.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let race = race.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(document_lookup(10));
            }
            std::fs::write(race, b"host-created").unwrap();
            Ok(file_part(vec![7; 10]))
        }
    });
    let (account, document) = source(&client).await;
    let failure = account
        .download_document_to(&document, &output, DownloadOptions::default())
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::File);
    assert_eq!(failure.outcome(), Outcome::Unknown);
    assert_eq!(std::fs::read(output).unwrap(), b"host-created");
    assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tg-download-")
    }));
    client.close().await.unwrap();
}

#[tokio::test]
async fn aligned_download_checks_eof_instead_of_accepting_a_truncated_metadata_size() {
    for extra in [false, true] {
        let (_directory, stored) = session().await;
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = reads.clone();
        let client = fixture(
            stored,
            ClientOptions::default().max_response_bytes(8192),
            move |_, bytes| {
                let calls = calls.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    if bytes[..4]
                        == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                    {
                        return Ok(document_lookup(4096));
                    }
                    let request = tl::functions::upload::GetFile::from_bytes(&bytes[4..]).unwrap();
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(file_part(if request.offset == 0 || extra {
                        vec![7; 4096]
                    } else {
                        vec![]
                    }))
                }
            },
        );
        let (account, document) = source(&client).await;
        let result = account
            .download_document(&document, DownloadOptions::default())
            .await;
        if extra {
            assert_eq!(result.unwrap_err().kind(), ClientErrorKind::Integrity);
        } else {
            assert_eq!(result.unwrap().bytes(), vec![7; 4096]);
        }
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_file_migration_is_verified_once_and_never_changes_home() {
    for again in [false, true] {
        let (_directory, stored) = session().await;
        let checks = Arc::new(AtomicUsize::new(0));
        let verified = checks.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = reads.clone();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let client = fixture(
            stored,
            ClientOptions::default().max_response_bytes(8192),
            move |dc, bytes| {
                let verified = verified.clone();
                let calls = calls.clone();
                let barrier = barrier.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        assert!(dc == 2 || dc == 3);
                        if dc == 3 {
                            verified.fetch_add(1, Ordering::SeqCst);
                        }
                        return Ok(reply);
                    }
                    if bytes[..4]
                        == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                    {
                        assert_eq!(dc, 2);
                        return Ok(document_lookup(5000));
                    }
                    let request = tl::functions::upload::GetFile::from_bytes(&bytes[4..]).unwrap();
                    calls.fetch_add(1, Ordering::SeqCst);
                    if dc == 2 {
                        barrier.wait().await;
                        return Err(rpc(303, "FILE_MIGRATE", Some(3)));
                    }
                    assert_eq!(dc, 3);
                    if again {
                        return Err(rpc(303, "FILE_MIGRATE", Some(4)));
                    }
                    Ok(file_part(vec![
                        7;
                        if request.offset == 0 { 4096 } else { 904 }
                    ]))
                }
            },
        );
        let (account, document) = source(&client).await;
        let result = account
            .download_document(&document, DownloadOptions::default())
            .await;
        if again {
            assert_eq!(result.unwrap_err().outcome(), Outcome::Rejected);
        } else {
            assert_eq!(result.unwrap().bytes(), vec![7; 5000]);
        }
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 4);
        client.verify_account(expected()).await.unwrap();
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn failed_read_drains_admitted_parts_before_releasing_capacity() {
    let (_directory, stored) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let blocked = release.clone();
    let client = fixture(
        stored,
        ClientOptions::default()
            .max_response_bytes(8192)
            .max_pending(1),
        move |_, bytes| {
            let observed = observed.clone();
            let blocked = blocked.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(document_lookup(5000));
                }
                let request = tl::functions::upload::GetFile::from_bytes(&bytes[4..]).unwrap();
                if request.offset == 0 {
                    return Err(rpc(500, "private read error", None));
                }
                observed.notify_one();
                blocked.notified().await;
                Ok(file_part(vec![7; 904]))
            }
        },
    );
    let (account, document) = source(&client).await;
    let operation = tokio::spawn(async move {
        account
            .download_document(&document, DownloadOptions::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    assert!(!operation.is_finished());
    assert!(matches!(client.reserve(), Err(error) if error.kind() == ClientErrorKind::Capacity));
    release.notify_one();
    assert_eq!(
        operation.await.unwrap().unwrap_err().kind(),
        ClientErrorKind::Rpc
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn dropped_download_caller_does_not_release_owned_file_work() {
    let (directory, stored) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let blocked = release.clone();
    let client = fixture(
        stored,
        ClientOptions::default().max_pending(1),
        move |_, bytes| {
            let observed = observed.clone();
            let blocked = blocked.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
                {
                    return Ok(document_lookup(10));
                }
                observed.notify_one();
                blocked.notified().await;
                Ok(file_part(vec![7; 10]))
            }
        },
    );
    let (account, document) = source(&client).await;
    let path = directory.path().join("owned-result.bin");
    let output = path.clone();
    let operation = tokio::spawn(async move {
        account
            .download_document_to(&document, path, DownloadOptions::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    operation.abort();
    let _ = operation.await;
    assert!(matches!(client.reserve(), Err(error) if error.kind() == ClientErrorKind::Capacity));
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if output.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close().await.unwrap();
    assert_eq!(std::fs::read(output).unwrap(), vec![7; 10]);
}

#[tokio::test]
async fn timed_out_download_fences_client_and_cleans_unpublished_destination() {
    let (directory, stored) = session().await;
    let client = fixture(
        stored,
        ClientOptions::default().request_timeout(Duration::from_millis(100)),
        |_, bytes| async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes() {
                return Ok(document_lookup(10));
            }
            std::future::pending().await
        },
    );
    let (account, document) = source(&client).await;
    let output = directory.path().join("timed-out.bin");
    let error = account
        .download_document_to(&document, &output, DownloadOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::Timeout);
    assert_eq!(error.outcome(), Outcome::Unknown);
    tokio::time::timeout(Duration::from_secs(1), client.close())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.exists());
    assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tg-download-")
    }));
}

#[tokio::test]
async fn download_preflight_rejects_foreign_capability_and_invalid_limits_before_io() {
    let (_directory, stored) = session().await;
    let client = fixture(stored, ClientOptions::default(), |_, bytes| async move {
        if let Some(reply) = self_reply(&bytes) {
            return Ok(reply);
        }
        assert_eq!(
            bytes[..4],
            tl::functions::messages::GetMessages::CONSTRUCTOR_ID.to_le_bytes()
        );
        Ok(document_lookup(10))
    });
    let (account, document) = source(&client).await;
    for options in [
        DownloadOptions::default().max_bytes(9),
        DownloadOptions::default().parallelism(0),
        DownloadOptions::default().parallelism(9),
    ] {
        assert_eq!(
            account
                .download_document(&document, options)
                .await
                .unwrap_err()
                .outcome(),
            Outcome::NotSent
        );
    }
    assert_eq!(
        account
            .download_document_to(&document, "relative.bin", DownloadOptions::default())
            .await
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    let (_other_directory, other_stored) = session().await;
    let other = fixture(other_stored, ClientOptions::default(), |_, _| async {
        panic!("foreign capability dispatched")
    });
    assert_eq!(
        other
            .account(expected())
            .download_document(&document, DownloadOptions::default())
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidPeer
    );
    client.inner.auth_epoch.fetch_add(1, Ordering::AcqRel);
    assert_eq!(
        account
            .download_document(&document, DownloadOptions::default())
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidPeer
    );
    client.close().await.unwrap();
    other.close().await.unwrap();
}
