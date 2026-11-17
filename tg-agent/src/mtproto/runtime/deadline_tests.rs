use super::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_lock_wait_does_not_extend_a_durable_write_lease() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        dir.path().join("deadline.sqlite"),
        journal::JournalOptions::default().lease_duration(Duration::from_millis(200)),
    )
    .await
    .unwrap();
    let expected = ProfileIdentity::mtproto_user(42).unwrap();
    journal.bind_profile("personal", expected).await.unwrap();
    let principal = Principal::new("alice").unwrap();
    let peer = PeerId::new(PeerKind::User, 7).unwrap();
    let policy = PolicyStore::default();
    policy
        .replace_mtproto(
            &principal,
            "personal",
            Grant::new(expected, Budget::new("quota").unwrap())
                .unwrap()
                .allow(Action::SendMessage)
                .peers(PeerScope::only([peer])),
        )
        .unwrap();
    let request = Request::new(
        "personal",
        Some("lease".into()),
        Call::SendMessage(SendMessageArgs {
            peer,
            text: "hello".into(),
            disable_notification: false,
        }),
    )
    .unwrap();
    let auth = Authorization {
        policy: policy.clone(),
        principal,
        request: request.clone(),
        approval: None,
        expected,
        scope: None,
        reference: None,
        file: None,
    };
    let scope = Scope {
        account: expected,
        action: Action::SendMessage,
        peer: Some(peer),
        file: None,
        reference_hash: None,
    };
    let intent = OperationRequest::new(
        "personal",
        auth.principal.as_str(),
        "lease",
        Action::SendMessage.name(),
        &serde_json::to_value(&request).unwrap(),
    )
    .unwrap();
    let Preparation::Ready(prepared) = journal
        .prepare_mtproto(intent, scope.clone())
        .await
        .unwrap()
    else {
        panic!("ready");
    };
    let admission = auth.clone();
    let flight = journal
        .begin_mtproto(prepared, scope, move |t| admission.budget(t))
        .await
        .unwrap();
    let (held, ready) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let gate = std::thread::spawn(move || {
        policy.with_exclusive_test_lock(|| {
            held.send(()).unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(5));
        })
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let attempt = tokio::spawn(async move {
        let _ = started.send(());
        let deadline = rpc_deadline(&flight, Duration::from_millis(50), &auth);
        (flight, deadline)
    });
    waiting.await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = release.send(());
    gate.join().unwrap();
    let (flight, result) = attempt.await.unwrap();
    assert_eq!(result.unwrap_err().kind(), ExecutorErrorKind::Capacity);
    assert_eq!(
        journal
            .complete_fenced(flight, Completion::executor_not_started())
            .await
            .unwrap()
            .state(),
        journal::OperationState::Prepared
    );
    journal.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_expiry_is_anchored_before_policy_wait_and_atomic_begin_rejects_expired_bytes() {
    for before_begin in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            dir.path().join("file-deadline.sqlite"),
            journal::JournalOptions::default(),
        )
        .await
        .unwrap();
        let expected = ProfileIdentity::mtproto_user(42).unwrap();
        journal.bind_profile("personal", expected).await.unwrap();
        let principal = Principal::new("alice").unwrap();
        let peer = PeerId::new(PeerKind::User, 7).unwrap();
        let policy = PolicyStore::default();
        policy
            .replace_mtproto(
                &principal,
                "personal",
                Grant::new(expected, Budget::new("files").unwrap())
                    .unwrap()
                    .allow(Action::SendDocument)
                    .peers(PeerScope::only([peer]))
                    .files(["file_unit"]),
            )
            .unwrap();
        let file = RegisteredFile {
            handle: "file_unit".into(),
            profile: "personal".into(),
            principal: "alice".into(),
            sha256: [0; 32],
            byte_size: 3,
            filename: "private.bin".into(),
            expires_at_ms: now().unwrap() + if before_begin { 400 } else { 2000 },
        };
        let request = Request::new(
            "personal",
            Some("file".into()),
            Call::SendDocument(SendDocumentArgs {
                peer,
                file_handle: file.handle.clone(),
                caption: None,
            }),
        )
        .unwrap();
        let auth = Authorization {
            policy: policy.clone(),
            principal,
            request: request.clone(),
            approval: None,
            expected,
            scope: None,
            reference: None,
            file: Some(file.clone()),
        };
        let scope = Scope {
            account: expected,
            action: Action::SendDocument,
            peer: Some(peer),
            reference_hash: None,
            file: Some(journal::authorization::FileScope {
                handle: file.handle.clone(),
                sha256: file.sha256,
                bytes: file.byte_size,
            }),
        };
        let intent = OperationRequest::new(
            "personal",
            "alice",
            "file",
            Action::SendDocument.name(),
            &serde_json::to_value(request).unwrap(),
        )
        .unwrap();
        let Preparation::Ready(prepared) = journal
            .prepare_mtproto(intent, scope.clone())
            .await
            .unwrap()
        else {
            panic!("ready")
        };
        let proof = prepared.clone();
        let flight = if before_begin {
            None
        } else {
            let admission = auth.clone();
            Some(
                journal
                    .begin_mtproto(prepared.clone(), scope.clone(), move |time| {
                        admission.write_budget(time)
                    })
                    .await
                    .unwrap(),
            )
        };
        let expected_deadline =
            Instant::now() + file_store::remaining(file.expires_at_ms, now().unwrap()).unwrap();
        let (held, ready) = std::sync::mpsc::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let gate = std::thread::spawn(move || {
            policy.with_exclusive_test_lock(|| {
                held.send(()).unwrap();
                let _ = waiting.recv_timeout(Duration::from_secs(5));
            })
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let (started, entered) = tokio::sync::oneshot::channel();
        let db = journal.clone();
        let attempt = tokio::spawn(async move {
            let _ = started.send(());
            if let Some(flight) = flight {
                let deadline = rpc_deadline(&flight, Duration::from_millis(50), &auth).unwrap();
                db.complete_fenced(flight, Completion::executor_not_started())
                    .await
                    .unwrap();
                Some(deadline)
            } else {
                let failure = db
                    .begin_mtproto(prepared, scope, move |time| auth.write_budget(time))
                    .await
                    .unwrap_err();
                assert_eq!(failure.kind(), JournalErrorKind::Denied);
                db.release_prepared(proof).await.unwrap();
                None
            }
        });
        entered.await.unwrap();
        tokio::time::sleep(Duration::from_millis(if before_begin { 500 } else { 100 })).await;
        let _ = release.send(());
        gate.join().unwrap();
        if let Some(deadline) = attempt.await.unwrap() {
            assert!(deadline <= expected_deadline + Duration::from_millis(20));
        } else {
            let usage = journal
                .run(|c, _, _| {
                    Ok((
                        c.query_row(
                            "SELECT COALESCE(SUM(upload_bytes),0) FROM executor_budgets",
                            [],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap(),
                        c.query_row("SELECT admitted FROM mtproto_operation_scopes", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap(),
                    ))
                })
                .await
                .unwrap();
            assert_eq!(
                usage,
                (0, 0),
                "expired authorization must roll back both admission and upload quota"
            );
        }
        journal.close().await.unwrap();
    }
}
