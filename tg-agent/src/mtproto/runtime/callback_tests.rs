use super::*;
use tg_mtproto::{
    ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl::Serializable,
};
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn cursor(profile: &str) -> updates::Cursor {
    use base64::Engine;
    format!(
        "tgmt1.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({"profile":profile,"sequence":1})).unwrap()
        )
    )
    .parse()
    .unwrap()
}
struct Fixture {
    _dir: tempfile::TempDir,
    journal: Journal,
    client: tg_mtproto::Client,
    executor: Executor,
    profile: Arc<Profile>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            dir.path().join("registry.sqlite"),
            journal::JournalOptions::default(),
        )
        .await
        .unwrap();
        let expected = ProfileIdentity::mtproto_bot(42).unwrap();
        let feed = updates::Feed::open(
            journal.clone(),
            "personal",
            expected,
            updates::Options::default(),
        )
        .await
        .unwrap();
        let session = Session::open(
            dir.path().join("session.sqlite"),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .unwrap();
        let client = test_kit::client(session, ClientOptions::default(), |_, _| async {
            Ok(true.to_bytes())
        })
        .unwrap();
        let policy = PolicyStore::default();
        let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
        executor.register_profile(feed, client.clone()).unwrap();
        let req =
            Request::new("personal", None, Call::GetMe(crate::executor::EmptyArgs {})).unwrap();
        let profile = executor.worker(&req).unwrap().profile;
        Self {
            _dir: dir,
            journal,
            client,
            executor,
            profile,
        }
    }
    fn observation(&self) -> updates::CallbackObservation {
        updates::CallbackObservation {
            cursor: cursor("personal"),
            query: 0,
            peer: Some(PeerId::new(PeerKind::User, 7).unwrap()),
            received_at_ms: now().unwrap(),
            origin: self.journal.origin_token(),
            observed: Some(Instant::now()),
        }
    }
    async fn close(&self) {
        self.executor.close().await.unwrap();
        self.client.close().await.unwrap();
        self.journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn callback_candidates_require_same_origin_profile_bot_and_original_observation_time() {
    let f = Fixture::new().await;
    let options = ExecutorOptions::default();
    let mut observation = f.observation();
    let time = observation.received_at_ms;
    let context = candidate(&f.profile, &observation, &options, time).unwrap();
    assert_eq!(context.query, 0);
    observation.observed = None;
    assert_eq!(
        candidate(&f.profile, &observation, &options, time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    let wall = options
        .clone()
        .callback_clock_policy(CallbackClockPolicy::TrustedWallClock);
    candidate(&f.profile, &observation, &wall, time).unwrap();
    observation.origin = Arc::from("foreign");
    assert_eq!(
        candidate(&f.profile, &observation, &wall, time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    observation = f.observation();
    observation.cursor = cursor("foreign");
    assert_eq!(
        candidate(&f.profile, &observation, &options, time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    observation = f.observation();
    observation.observed = Some(Instant::now() - Duration::from_secs(61));
    assert_eq!(
        candidate(&f.profile, &observation, &options, time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceExpired
    );
    f.close().await;
}

#[tokio::test]
async fn canonical_reference_reuse_rejects_changed_scope_or_receipt_and_is_private_to_owner() {
    let f = Fixture::new().await;
    let options = ExecutorOptions::default();
    let observation = f.observation();
    let time = observation.received_at_ms;
    let mut registry = Registry::default();
    let first = registry
        .register(
            &actor(),
            "personal",
            candidate(&f.profile, &observation, &options, time).unwrap(),
            10,
            time,
        )
        .unwrap();
    let again = registry
        .register(
            &actor(),
            "personal",
            candidate(&f.profile, &observation, &options, time).unwrap(),
            10,
            time,
        )
        .unwrap();
    assert_eq!(first.reference(), again.reference());
    assert!(first.reference().starts_with("mtcb_"));
    assert_eq!(first.reference().len(), 37);
    let bob = Principal::new("bob").unwrap();
    assert_eq!(
        registry
            .lookup(&bob, "personal", first.reference(), time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    assert_eq!(
        registry
            .lookup(&actor(), "other", first.reference(), time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    assert_eq!(
        registry
            .lookup(&actor(), "personal", "0", time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    let other = registry
        .register(
            &bob,
            "personal",
            candidate(&f.profile, &observation, &options, time).unwrap(),
            10,
            time,
        )
        .unwrap();
    assert_ne!(first.reference(), other.reference());
    assert_eq!(
        first.context.scope.reference_hash,
        other.context.scope.reference_hash
    );
    let mut changed = observation.clone();
    changed.peer = Some(PeerId::new(PeerKind::Channel, 7).unwrap());
    assert_eq!(
        registry
            .register(
                &actor(),
                "personal",
                candidate(&f.profile, &changed, &options, time).unwrap(),
                10,
                time
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    changed = observation.clone();
    changed.received_at_ms += 1;
    assert_eq!(
        registry
            .register(
                &actor(),
                "personal",
                candidate(&f.profile, &changed, &options, time + 1).unwrap(),
                10,
                time + 1
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert!(!format!("{first:?}").contains(first.reference()));
    f.close().await;
}

#[tokio::test]
async fn reference_capacity_pruning_and_clock_high_water_do_not_refresh_old_deadlines() {
    let f = Fixture::new().await;
    let options = ExecutorOptions::default();
    let observation = f.observation();
    let time = observation.received_at_ms;
    let mut registry = Registry::default();
    let first = registry
        .register(
            &actor(),
            "personal",
            candidate(&f.profile, &observation, &options, time).unwrap(),
            1,
            time,
        )
        .unwrap();
    let mut next = observation.clone();
    next.query = 1;
    assert_eq!(
        registry
            .register(
                &actor(),
                "personal",
                candidate(&f.profile, &next, &options, time).unwrap(),
                1,
                time
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    registry
        .entries
        .values_mut()
        .next()
        .unwrap()
        .context
        .deadline
        .instant = Instant::now() - Duration::from_millis(1);
    assert_eq!(registry.prune(time), 1);
    assert!(registry.ids.is_empty());
    assert_eq!(
        registry
            .lookup(&actor(), "personal", first.reference(), time)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    registry
        .register(
            &actor(),
            "personal",
            candidate(&f.profile, &next, &options, time).unwrap(),
            1,
            time,
        )
        .unwrap();
    assert_eq!(registry.prune(time + 30_000), 1);
    assert_eq!(
        registry
            .register(
                &actor(),
                "personal",
                candidate(&f.profile, &observation, &options, time).unwrap(),
                1,
                time
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceExpired
    );
    f.close().await;
}

#[tokio::test]
async fn callback_intent_hash_uses_semantic_target_and_original_time_while_approval_keeps_the_alias()
 {
    let f = Fixture::new().await;
    let options = ExecutorOptions::default();
    let mut observation = f.observation();
    observation.query = 1239876543210;
    let time = observation.received_at_ms;
    let context = candidate(&f.profile, &observation, &options, time).unwrap();
    let req = |alias: &str, text: &str| {
        Request::new(
            "personal",
            Some("stable".into()),
            Call::AnswerCallbackQuery(AnswerCallbackQueryArgs {
                reference: alias.into(),
                text: Some(text.into()),
                show_alert: false,
            }),
        )
        .unwrap()
    };
    let first = req("mtcb_first", "hello");
    let second = req("mtcb_second", "hello");
    assert_ne!(first.fingerprint(), second.fingerprint());
    assert_eq!(
        payload(&first, &context).unwrap(),
        payload(&second, &context).unwrap()
    );
    assert_ne!(
        payload(&first, &context).unwrap(),
        payload(&req("mtcb_first", "different"), &context).unwrap()
    );
    let mut changed = context.clone();
    changed.deadline.received_at_ms += 1;
    assert_ne!(
        payload(&first, &context).unwrap(),
        payload(&first, &changed).unwrap()
    );
    let encoded = serde_json::to_string(&payload(&first, &context).unwrap()).unwrap();
    assert!(!encoded.contains("1239876543210"));
    assert!(!encoded.contains("mtcb_first"));
    f.close().await;
}

#[tokio::test]
async fn a_cloned_callback_context_cannot_revive_after_shared_registry_wall_expiry() {
    let f = Fixture::new().await;
    let options = ExecutorOptions::default();
    let observation = f.observation();
    let time = observation.received_at_ms;
    let mut registry = Registry::default();
    let reference = registry
        .register(
            &actor(),
            "personal",
            candidate(&f.profile, &observation, &options, time).unwrap(),
            10,
            time,
        )
        .unwrap();
    let copied = registry
        .lookup(&actor(), "personal", reference.reference(), time)
        .unwrap()
        .context;
    assert_eq!(registry.prune(time + 30_000), 1);
    assert_eq!(
        copied.deadline.remaining(time).unwrap_err().kind(),
        ExecutorErrorKind::ReferenceExpired,
        "a buffered authorization must retain irreversible expiry across wall rollback"
    );
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callback_policy_wait_cannot_disclose_after_grant_or_approval_expiry() {
    let f = Fixture::new().await;
    for approval_required in [false, true] {
        let time = now().unwrap();
        let observation = f.observation();
        let context = candidate(
            &f.profile,
            &observation,
            &ExecutorOptions::default(),
            observation.received_at_ms,
        )
        .unwrap();
        let policy = f.executor.policy.clone();
        let principal = actor();
        let mut grant = Grant::new(f.profile.expected, Budget::new("expiry").unwrap())
            .unwrap()
            .allow(Action::AnswerCallbackQuery)
            .peers(PeerScope::only(context.scope.peer));
        if approval_required {
            grant = grant.require_approval(Action::AnswerCallbackQuery);
        } else {
            grant = grant.expires_at_ms(time + 100);
        }
        policy
            .replace_mtproto(&principal, "personal", grant)
            .unwrap();
        let request = Request::new(
            "personal",
            Some("expiry".into()),
            Call::AnswerCallbackQuery(AnswerCallbackQueryArgs {
                reference: "known_alias".into(),
                text: None,
                show_alert: false,
            }),
        )
        .unwrap();
        let approval = if approval_required {
            Some(
                policy
                    .approve_mtproto_with_scope(
                        &principal,
                        &request,
                        f.profile.expected,
                        PolicyTime::Fixed(time),
                        time + 100,
                        Some(&context.scope.policy_scope()),
                    )
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
            expected: f.profile.expected,
            scope: Some(context.scope.clone()),
            reference: Some(context),
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
    f.close().await;
}

#[tokio::test]
async fn buffered_callback_results_recheck_revision_and_reference_expiry_at_caller_disclosure() {
    let f = Fixture::new().await;
    for write in [false, true] {
        for expire in [false, true] {
            let observation = f.observation();
            let context = candidate(
                &f.profile,
                &observation,
                &ExecutorOptions::default(),
                now().unwrap(),
            )
            .unwrap();
            let policy = f.executor.policy.clone();
            let grant = || {
                Grant::new(f.profile.expected, Budget::new("buffered").unwrap())
                    .unwrap()
                    .allow(Action::ReadUpdates)
                    .allow(Action::AnswerCallbackQuery)
                    .peers(PeerScope::only(context.scope.peer))
            };
            policy
                .replace_mtproto(&actor(), "personal", grant())
                .unwrap();
            let call = if write {
                Call::AnswerCallbackQuery(AnswerCallbackQueryArgs {
                    reference: "buffered".into(),
                    text: None,
                    show_alert: false,
                })
            } else {
                Call::ReadUpdates(crate::executor::ReadUpdatesArgs {
                    cursor: None,
                    limit: 100,
                })
            };
            let request = Request::new("personal", write.then(|| "buffered".into()), call).unwrap();
            let auth = Authorization {
                policy: policy.clone(),
                principal: actor(),
                request,
                approval: None,
                expected: f.profile.expected,
                scope: write.then(|| context.scope.clone()),
                reference: write.then(|| context.clone()),
            };
            let revision = auth.checked(now().unwrap()).unwrap().0.grant_revision;
            // The output has passed worker authorization and is buffered in oneshot.
            let buffered = Authorized {
                auth,
                revision,
                value: ExecutionResult::account(f.profile.expected),
            };
            if expire && write {
                context
                    .deadline
                    .clock
                    .observe(context.deadline.expires_at_ms);
            } else {
                policy
                    .replace_mtproto(&actor(), "personal", grant())
                    .unwrap();
            }
            let error = buffered.disclose().unwrap_err();
            assert_eq!(
                error.kind(),
                if expire && write {
                    ExecutorErrorKind::ReferenceExpired
                } else {
                    ExecutorErrorKind::Denied
                }
            );
            assert_eq!(error.outcome(), write.then_some(EffectOutcome::Unknown));
        }
    }
    f.close().await;
}
