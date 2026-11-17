use super::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_runtime_rechecks_grant_and_approval_time_after_waiting_for_the_policy_lock() {
    for approval_required in [false, true] {
        let policy = PolicyStore::default();
        let principal = Principal::new("alice").unwrap();
        let time = now().unwrap();
        let mut grant = Grant::new(0, Budget::new("expiry").unwrap()).allow(Action::GetMe);
        if approval_required {
            grant = grant.require_approval(Action::GetMe);
        } else {
            grant = grant.expires_at_ms(time + 100);
        }
        policy.replace(&principal, "primary", grant).unwrap();
        let request = Request::new("primary", None, ActionCall::GetMe(EmptyArgs {})).unwrap();
        let approval = if approval_required {
            Some(
                policy
                    .approve(&principal, &request, 0, time, time + 100)
                    .unwrap(),
            )
        } else {
            None
        };
        let auth = Authorization {
            policy: policy.clone(),
            principal,
            request,
            approval,
            bot_id: 0,
            scope: None,
            reference: None,
            file: None,
        };
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
        let job = tokio::spawn(async move {
            let _ = started.send(());
            auth.checked(time)
        });
        waiting.await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = release.send(());
        gate.join().unwrap();
        assert_eq!(
            job.await.unwrap().unwrap_err().kind(),
            if approval_required {
                ExecutorErrorKind::ApprovalRequired
            } else {
                ExecutorErrorKind::Denied
            }
        );
    }
}
