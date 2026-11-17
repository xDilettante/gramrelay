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
