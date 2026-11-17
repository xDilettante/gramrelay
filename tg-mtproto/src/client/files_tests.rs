use super::*;
use crate::SendDocument;
#[path = "download_tests.rs"]
mod download;

#[test]
fn document_names_mime_captions_and_ids_reject_invalid_input_without_io() {
    for mime in [
        "!/octet-stream",
        "application/.bin",
        "application/",
        "/bin",
        "text/plain; charset=utf-8",
        "text/pläin",
    ] {
        assert_eq!(
            SendDocument::bytes("private.bin", mime, vec![1])
                .unwrap_err()
                .outcome(),
            Outcome::NotSent
        );
    }
    for name in [
        "",
        "../private",
        "a\\b",
        "a:b",
        "a\0b",
        "a.",
        "a ",
        &"a".repeat(256),
    ] {
        assert!(SendDocument::bytes(name, "application/octet-stream", vec![1]).is_err());
    }
    assert!(SendDocument::bytes("file", "application/octet-stream", vec![]).is_err());
    assert!(SendDocument::path("relative-private", "file", "application/octet-stream").is_err());
    assert!(
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .caption("😀".repeat(513))
            .is_err()
    );
    assert!(
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .with_random_id(0)
            .is_err()
    );
    let valid = SendDocument::bytes("file", "application/vnd.test+json", vec![1])
        .unwrap()
        .caption("😀".repeat(512))
        .unwrap();
    assert_ne!(valid.random_id(), 0);
    assert!(
        SendDocument::bytes(
            "file",
            format!("{}/{}", "a".repeat(127), "b".repeat(127)),
            vec![1]
        )
        .is_ok()
    );
}

fn document_media(size: u64) -> tl::enums::MessageMedia {
    let mut bytes = tl::types::MessageMediaDocument::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&tl::types::Document::CONSTRUCTOR_ID.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&123i64.to_le_bytes());
    bytes.extend_from_slice(&987654321i64.to_le_bytes());
    bytes.extend_from_slice(&b"private file ref".to_vec().to_bytes());
    bytes.extend_from_slice(&123i32.to_le_bytes());
    bytes.extend_from_slice(&"application/octet-stream".to_owned().to_bytes());
    bytes.extend_from_slice(&(size as i64).to_le_bytes());
    bytes.extend_from_slice(&2i32.to_le_bytes());
    bytes.extend_from_slice(
        &vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "private.bin".into(),
            },
        )]
        .to_bytes(),
    );
    tl::enums::MessageMedia::from_bytes(&bytes).unwrap()
}
fn document_reply(size: u64) -> Vec<u8> {
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id: 99,
        pts: 10,
        pts_count: 1,
        date: 123,
        media: Some(document_media(size)),
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}

#[tokio::test]
async fn small_document_uses_real_parts_checksum_and_one_verified_media_send() {
    let (_directory, stored) = session().await;
    let parts = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    let records = parts.clone();
    let data: Vec<u8> = (0..6000).map(|i| (i % 251) as u8).collect();
    let expected_bytes = data.clone();
    let client = fixture(
        stored,
        ClientOptions::default().max_request_bytes(4096),
        move |_, bytes| {
            let records = records.clone();
            let expected_bytes = expected_bytes.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes() {
                    let request =
                        tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
                    assert!(request.bytes.len() <= 2048);
                    records
                        .lock()
                        .unwrap()
                        .insert(request.file_part, request.bytes);
                    return Ok(true.to_bytes());
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMedia::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request = tl::functions::messages::SendMedia::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.random_id, 1234567);
                assert_eq!(request.message, "private caption");
                assert!(!request.allow_paid_floodskip && request.allow_paid_stars.is_none());
                let tl::enums::InputMedia::UploadedDocument(media) = request.media else {
                    panic!()
                };
                assert!(media.force_file);
                let tl::enums::InputFile::File(file) = media.file else {
                    panic!()
                };
                assert_eq!(file.parts, 3);
                assert_eq!(file.md5_checksum, "15c4711e00f69307d777469d7fbb061c");
                let restored: Vec<u8> = records
                    .lock()
                    .unwrap()
                    .values()
                    .flat_map(|part| part.iter().copied())
                    .collect();
                assert_eq!(restored, expected_bytes);
                Ok(document_reply(6000))
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let intent = SendDocument::bytes("private.bin", "application/octet-stream", data)
        .unwrap()
        .with_random_id(1234567)
        .unwrap()
        .caption("private caption")
        .unwrap();
    assert!(!format!("{intent:?}").contains("private"));
    let receipt = account.send_document(&peer, intent).await.unwrap();
    assert_eq!(receipt.message().message_id(), 99);
    assert_eq!(receipt.size(), 6000);
    use sha2::Digest;
    let wanted: [u8; 32] =
        sha2::Sha256::digest((0..6000).map(|i| (i % 251) as u8).collect::<Vec<_>>()).into();
    assert_eq!(receipt.sha256(), wanted);
    client.close().await.unwrap();
}

#[tokio::test]
async fn large_document_uses_fixed_big_parts_with_bounded_parallelism() {
    let (_directory, stored) = session().await;
    let size = 10 * 1024 * 1024 + 1;
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let count = Arc::new(AtomicUsize::new(0));
    let states = (active.clone(), peak.clone(), count.clone());
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let (active, peak, count) = states.clone();
        async move {
            if let Some(reply) = self_reply(&bytes) {
                return Ok(reply);
            }
            if bytes[..4] == tl::functions::upload::SaveBigFilePart::CONSTRUCTOR_ID.to_le_bytes() {
                let request =
                    tl::functions::upload::SaveBigFilePart::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(request.file_total_parts, 21);
                assert_eq!(
                    request.bytes.len(),
                    if request.file_part == 20 {
                        1
                    } else {
                        512 * 1024
                    }
                );
                assert!(request.bytes.iter().all(|byte| *byte == 7));
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                count.fetch_add(1, Ordering::SeqCst);
                return Ok(true.to_bytes());
            }
            let request = tl::functions::messages::SendMedia::from_bytes(&bytes[4..]).unwrap();
            let tl::enums::InputMedia::UploadedDocument(media) = request.media else {
                panic!()
            };
            assert!(matches!(media.file, tl::enums::InputFile::Big(file) if file.parts == 21));
            assert_eq!(active.load(Ordering::SeqCst), 0);
            assert_eq!(count.load(Ordering::SeqCst), 21);
            Ok(document_reply(size))
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let intent = SendDocument::bytes(
        "private.bin",
        "application/octet-stream",
        vec![7; size as usize],
    )
    .unwrap()
    .parallelism(3);
    assert_eq!(
        account.send_document(&peer, intent).await.unwrap().size(),
        size
    );
    assert_eq!(peak.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}

#[tokio::test]
async fn failed_parts_drain_admitted_work_and_keep_parent_capacity_before_returning() {
    for mode in 0..3 {
        let (_directory, stored) = session().await;
        let second_started = Arc::new(tokio::sync::Notify::new());
        let failure_ready = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let state = (
            second_started.clone(),
            failure_ready.clone(),
            release.clone(),
            calls.clone(),
        );
        let client = fixture(
            stored,
            ClientOptions::default()
                .max_pending(1)
                .max_request_bytes(4096),
            move |_, bytes| {
                let (second_started, failure_ready, release, calls) = state.clone();
                async move {
                    if let Some(reply) = self_reply(&bytes) {
                        return Ok(reply);
                    }
                    assert_eq!(
                        bytes[..4],
                        tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes()
                    );
                    let request =
                        tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
                    calls.fetch_add(1, Ordering::SeqCst);
                    match request.file_part {
                        0 => {
                            second_started.notified().await;
                            failure_ready.notify_one();
                            if mode == 0 {
                                Ok(false.to_bytes())
                            } else {
                                Err(grammers_mtsender::InvocationError::Rpc(
                                    grammers_mtsender::RpcError {
                                        code: if mode == 1 { 420 } else { 500 },
                                        name: if mode == 1 {
                                            "FLOOD_WAIT".into()
                                        } else {
                                            "PRIVATE_REMOTE_FAILURE".into()
                                        },
                                        value: (mode == 1).then_some(5),
                                        caused_by: None,
                                    },
                                ))
                            }
                        }
                        1 => {
                            second_started.notify_one();
                            release.notified().await;
                            Ok(true.to_bytes())
                        }
                        _ => panic!("no new part after observed failure"),
                    }
                }
            },
        );
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.inner.slots.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let task = tokio::spawn(async move {
            account
                .send_document(
                    &peer,
                    SendDocument::bytes("private.bin", "application/octet-stream", vec![7; 12000])
                        .unwrap()
                        .parallelism(2),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), failure_ready.notified())
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!task.is_finished());
        assert_eq!(
            client
                .invoke(&tl::functions::Ping { ping_id: 1 })
                .await
                .unwrap_err()
                .kind(),
            ClientErrorKind::Capacity
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        release.notify_one();
        let failure = task.await.unwrap().unwrap_err();
        assert_eq!(
            failure.outcome(),
            if mode == 2 {
                Outcome::Unknown
            } else {
                Outcome::Rejected
            }
        );
        assert!(!format!("{failure:?}").contains("PRIVATE_REMOTE_FAILURE"));
        if mode == 1 {
            assert_eq!(failure.retry_after_seconds(), Some(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn path_upload_detects_mid_upload_changes_and_never_dispatches_the_final_message() {
    use std::io::{Seek, SeekFrom, Write};
    let (directory, stored) = session().await;
    let path = directory.path().join("private-source.bin");
    std::fs::write(&path, vec![7; 6000]).unwrap();
    let source = path.clone();
    let client = fixture(
        stored,
        ClientOptions::default().max_request_bytes(4096),
        move |_, bytes| {
            let source = source.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes()
                );
                let request = tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
                if request.file_part == 0 {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(source)
                        .unwrap();
                    file.seek(SeekFrom::Start(2048)).unwrap();
                    file.write_all(&[9; 100]).unwrap();
                }
                Ok(true.to_bytes())
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let failure = account
        .send_document(
            &peer,
            SendDocument::path(path, "private.bin", "application/octet-stream")
                .unwrap()
                .parallelism(1),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ClientErrorKind::Integrity);
    assert!(!format!("{failure:?}").contains("private"));
    client.close().await.unwrap();
}

#[tokio::test]
async fn frozen_path_success_and_expected_digest_preflight_share_the_owned_pipeline() {
    use sha2::Digest;
    for wrong_digest in [false, true] {
        let (directory, stored) = session().await;
        let path = directory.path().join("private-source.bin");
        std::fs::write(&path, [7; 20]).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let seen = seen.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert!(
                    !wrong_digest,
                    "wrong source digest must stop before part RPC"
                );
                seen.fetch_add(1, Ordering::SeqCst);
                if bytes[..4] == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes() {
                    Ok(true.to_bytes())
                } else {
                    Ok(document_reply(20))
                }
            }
        });
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        let wanted: [u8; 32] = sha2::Sha256::digest([7; 20]).into();
        let intent = SendDocument::path(path, "private.bin", "application/octet-stream")
            .unwrap()
            .expected_sha256(if wrong_digest { [0; 32] } else { wanted });
        let result = account.send_document(&peer, intent).await;
        if wrong_digest {
            assert_eq!(result.unwrap_err().outcome(), Outcome::NotSent);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(result.unwrap().sha256(), wanted);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn document_receipts_require_matching_media_and_exact_full_update_mapping() {
    let full = |random_id| {
        let mut raw = message(99, 42, "", true);
        if let tl::enums::Message::Message(raw) = &mut raw {
            raw.media = Some(document_media(20));
        }
        updates(vec![
            mapped(99, random_id),
            tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
                message: raw,
                pts: 10,
                pts_count: 1,
            }),
        ])
    };
    let mut cases = vec![
        (document_reply(20), true),
        (full(1234567), true),
        (full(7654321), false),
        (document_reply(21), false),
        (sent(99), false),
    ];
    for change in 0..4 {
        let mut response = tl::enums::Updates::from_bytes(&document_reply(20)).unwrap();
        let tl::enums::Updates::UpdateShortSentMessage(sent) = &mut response else {
            unreachable!()
        };
        let Some(tl::enums::MessageMedia::Document(media)) = &mut sent.media else {
            unreachable!()
        };
        let Some(tl::enums::Document::Document(document)) = &mut media.document else {
            unreachable!()
        };
        match change {
            0 => document.mime_type = "image/png".into(),
            1 => {
                document.attributes = vec![tl::enums::DocumentAttribute::Filename(
                    tl::types::DocumentAttributeFilename {
                        file_name: "wrong.bin".into(),
                    },
                )]
            }
            2 => document.attributes.push(document.attributes[0].clone()),
            _ => {
                media.document = Some(tl::enums::Document::Empty(tl::types::DocumentEmpty {
                    id: 123,
                }))
            }
        }
        cases.push((response.to_bytes(), false));
    }
    for (response, success) in cases {
        let (_directory, stored) = session().await;
        let sends = Arc::new(AtomicUsize::new(0));
        let calls = sends.clone();
        let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
            let response = response.clone();
            let calls = calls.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes() {
                    return Ok(true.to_bytes());
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMedia::CONSTRUCTOR_ID.to_le_bytes()
                );
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(response)
            }
        });
        let account = client.account(expected());
        let peer = account
            .cached_peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap();
        let result = account
            .send_document(
                &peer,
                SendDocument::bytes("private.bin", "application/octet-stream", vec![7; 20])
                    .unwrap()
                    .with_random_id(1234567)
                    .unwrap(),
            )
            .await;
        if success {
            assert_eq!(result.unwrap().message().message_id(), 99);
        } else {
            let failure = result.unwrap_err();
            assert_eq!(
                (failure.kind(), failure.outcome()),
                (ClientErrorKind::Decode, Outcome::Unknown)
            );
        }
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        client.close().await.unwrap();
    }
}

#[tokio::test]
async fn upload_timeout_joins_all_pending_parts_without_a_final_send_or_replay() {
    let (_directory, stored) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let running = active.clone();
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let client = fixture(
        stored,
        ClientOptions::default()
            .max_pending(1)
            .max_request_bytes(4096)
            .request_timeout(Duration::from_millis(500)),
        move |_, bytes| {
            let seen = seen.clone();
            let running = running.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes()
                );
                seen.fetch_add(1, Ordering::SeqCst);
                running.fetch_add(1, Ordering::SeqCst);
                let _active = Active(running);
                std::future::pending().await
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let failure = account
        .send_document(
            &peer,
            SendDocument::bytes("private.bin", "application/octet-stream", vec![7; 40000]).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (failure.kind(), failure.outcome()),
        (ClientErrorKind::Timeout, Outcome::Unknown)
    );
    client.close().await.unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!((1..=4).contains(&calls.load(Ordering::SeqCst)));
}

#[tokio::test]
async fn dropped_caller_keeps_upload_admission_until_the_owned_send_finishes() {
    let (_directory, stored) = session().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let barrier = release.clone();
    let sent = Arc::new(tokio::sync::Notify::new());
    let finished = sent.clone();
    let client = fixture(
        stored,
        ClientOptions::default()
            .max_pending(1)
            .max_request_bytes(4096),
        move |_, bytes| {
            let signal = signal.clone();
            let barrier = barrier.clone();
            let finished = finished.clone();
            async move {
                if let Some(reply) = self_reply(&bytes) {
                    return Ok(reply);
                }
                if bytes[..4] == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID.to_le_bytes() {
                    signal.notify_one();
                    barrier.acquire().await.unwrap().forget();
                    return Ok(true.to_bytes());
                }
                assert_eq!(
                    bytes[..4],
                    tl::functions::messages::SendMedia::CONSTRUCTOR_ID.to_le_bytes()
                );
                finished.notify_one();
                Ok(document_reply(6000))
            }
        },
    );
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let caller = tokio::spawn(async move {
        account
            .send_document(
                &peer,
                SendDocument::bytes("private.bin", "application/octet-stream", vec![7; 6000])
                    .unwrap()
                    .parallelism(2),
            )
            .await
    });
    started.notified().await;
    caller.abort();
    let _ = caller.await;
    assert_eq!(
        client
            .invoke(&tl::functions::Ping { ping_id: 1 })
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::Capacity
    );
    release.add_permits(3);
    tokio::time::timeout(Duration::from_secs(2), sent.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.inner.slots.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn invalid_local_budgets_and_foreign_peers_start_no_source_work_or_rpc() {
    let (_directory, stored) = session().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen = checks.clone();
    let client = fixture(stored, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            assert_eq!(seen.fetch_add(1, Ordering::SeqCst), 0);
            Ok(self_reply(&bytes).expect("only initial identity lookup"))
        }
    });
    let account = client.account(expected());
    let peer = account
        .cached_peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    for intent in [
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .max_bytes(0),
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .max_bytes(4 * 1024 * 1024 * 1024 + 1),
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .parallelism(0),
        SendDocument::bytes("file", "text/plain", vec![1])
            .unwrap()
            .parallelism(9),
        SendDocument::bytes("file", "text/plain", vec![1; 10])
            .unwrap()
            .max_bytes(5),
    ] {
        assert_eq!(
            account
                .send_document(&peer, intent)
                .await
                .unwrap_err()
                .outcome(),
            Outcome::NotSent
        );
    }
    let (_directory2, stored2) = session().await;
    let other = fixture(stored2, ClientOptions::default(), |_, _| async {
        panic!("foreign peer must fail before I/O")
    });
    let intent = SendDocument::path(
        std::env::temp_dir().join("private-nonexistent-document"),
        "file",
        "text/plain",
    )
    .unwrap();
    assert_eq!(
        other
            .account(expected())
            .send_document(&peer, intent)
            .await
            .unwrap_err()
            .kind(),
        ClientErrorKind::InvalidPeer
    );
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    other.close().await.unwrap();
    client.close().await.unwrap();
}
