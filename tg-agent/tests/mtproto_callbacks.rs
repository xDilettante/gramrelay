#![cfg(feature = "mtproto-test-kit")]
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    executor::ExecutorErrorKind,
    journal::{Journal, JournalOptions, OperationState},
    mtproto::{
        Action, Budget, CallbackClockPolicy, Executor, ExecutorOptions, Grant, PeerId, PeerKind,
        PeerScope, PolicyStore, Principal, Request,
        updates::{Feed, Options, ReadOptions, Reader, StoredEvent},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, Client, ClientOptions, Session, SessionBinding,
    SessionKey, SessionOptions, UpdateMode, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};

fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn bob() -> Principal {
    Principal::new("bob").unwrap()
}
fn peer(id: i64) -> PeerId {
    PeerId::new(PeerKind::User, id).unwrap()
}
fn identity(bot: bool) -> ProfileIdentity {
    if bot {
        ProfileIdentity::mtproto_bot(42)
    } else {
        ProfileIdentity::mtproto_user(42)
    }
    .unwrap()
}
fn expected(bot: bool) -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: if bot {
            AccountKind::Bot
        } else {
            AccountKind::User
        },
    }
}
fn grant() -> Grant {
    Grant::new(identity(true), Budget::new("daily").unwrap())
        .unwrap()
        .allow(Action::ReadUpdates)
        .allow(Action::AnswerCallbackQuery)
        .allow(Action::GetOperation)
        .peers(PeerScope::only([peer(7)]))
}
fn request(tool: &str, args: Value, key: Option<&str>) -> Request {
    let mut v =
        json!({"schema_version":1,"profile":"personal","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        v["operation_id"] = json!(key);
    }
    Request::from_json(&serde_json::to_vec(&v).unwrap()).unwrap()
}
fn read() -> Request {
    request("mtproto_read_updates", json!({"limit":100}), None)
}
fn answer(reference: &str, key: &str) -> Request {
    request(
        "mtproto_answer_callback_query",
        json!({"reference":reference,"text":"PRIVATE_ANSWER","show_alert":true}),
        Some(key),
    )
}
fn lookup(key: &str) -> Request {
    request("mtproto_get_operation", json!({"operation_id":key}), None)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn user(bot: bool) -> tl::enums::User {
    let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&((1u32 << 10) | if bot { 1 << 14 } else { 0 }).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&42i64.to_le_bytes());
    if bot {
        v.extend_from_slice(&0i32.to_le_bytes());
    }
    tl::enums::User::from_bytes(&v).unwrap()
}
fn state(pts: i32) -> tl::enums::updates::State {
    tl::types::updates::State {
        pts,
        qts: 0,
        seq: 0,
        date: 100 + pts,
        unread_count: 0,
    }
    .into()
}
fn callback(query: i64, id: i64) -> tl::enums::Update {
    tl::types::UpdateBotCallbackQuery {
        query_id: query,
        user_id: 99,
        peer: tl::types::PeerUser { user_id: id }.into(),
        msg_id: 3,
        chat_instance: 888,
        data: Some(b"untrusted payload".to_vec()),
        game_short_name: None,
    }
    .into()
}
fn difference() -> Vec<u8> {
    tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
        new_messages: vec![],
        new_encrypted_messages: vec![],
        chats: vec![],
        users: vec![],
        state: state(2),
        other_updates: vec![
            callback(0, 7),
            callback(1, 9),
            tl::types::UpdateInlineBotCallbackQuery {
                query_id: 2,
                user_id: 99,
                msg_id: tl::types::InputBotInlineMessageId {
                    dc_id: 1,
                    id: 3,
                    access_hash: 987654321,
                }
                .into(),
                chat_instance: 888,
                data: None,
                game_short_name: None,
            }
            .into(),
        ],
    })
    .to_bytes()
}
#[derive(Default)]
struct Control {
    mode: AtomicUsize,
    writes: AtomicUsize,
    identities: AtomicUsize,
    delay_identity: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
async fn client(path: &std::path::Path, control: Arc<Control>, bot: bool) -> Client {
    let session = Session::open(
        path,
        SessionBinding::new("synthetic", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    test_kit::client(
        session,
        ClientOptions::default().request_timeout(Duration::from_millis(500)),
        move |_, bytes| {
            let c = control.clone();
            async move {
                let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
                    c.identities.fetch_add(1, Ordering::SeqCst);
                    if c.delay_identity.load(Ordering::SeqCst) {
                        c.entered.notify_one();
                        std::future::pending::<()>().await;
                    }
                    return Ok(vec![user(bot)].to_bytes());
                }
                if ctor == tl::functions::updates::GetState::CONSTRUCTOR_ID {
                    return Ok(state(1).to_bytes());
                }
                if ctor == tl::functions::updates::GetDifference::CONSTRUCTOR_ID {
                    return Ok(difference());
                }
                assert_eq!(
                    ctor,
                    tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID
                );
                let args =
                    tl::functions::messages::SetBotCallbackAnswer::from_bytes(&bytes[4..]).unwrap();
                assert!([0, 2].contains(&args.query_id));
                assert_eq!(args.message.as_deref(), Some("PRIVATE_ANSWER"));
                assert!(args.alert && args.url.is_none());
                assert_eq!(args.cache_time, 0);
                c.writes.fetch_add(1, Ordering::SeqCst);
                c.entered.notify_one();
                match c.mode.load(Ordering::SeqCst) {
                    1 => Ok(false.to_bytes()),
                    2 => Err(test_kit::InvocationError::Rpc(
                        tl::types::RpcError {
                            error_code: 400,
                            error_message: "PRIVATE_RPC".into(),
                        }
                        .into(),
                    )),
                    3 => std::future::pending().await,
                    4 => {
                        c.release.notified().await;
                        Ok(true.to_bytes())
                    }
                    _ => Ok(true.to_bytes()),
                }
            }
        },
    )
    .unwrap()
}
struct Fixture {
    dir: tempfile::TempDir,
    journal: Journal,
    feed: Feed,
    client: Client,
    policy: PolicyStore,
    executor: Executor,
    control: Arc<Control>,
    events: Vec<StoredEvent>,
}
impl Fixture {
    async fn new(options: ExecutorOptions) -> Self {
        Self::with_kind(options, true).await
    }
    async fn with_kind(options: ExecutorOptions, bot: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
            .await
            .unwrap();
        let feed = Feed::open(
            journal.clone(),
            "personal",
            identity(bot),
            Options::default(),
        )
        .await
        .unwrap();
        let control = Arc::new(Control::default());
        let client = client(&dir.path().join("session.sqlite"), control.clone(), bot).await;
        let mut source = feed.claim_source().await.unwrap();
        let mut receiver = client
            .account(expected(bot))
            .updates(UpdateMode::StartNow)
            .unwrap();
        for _ in 0..2 {
            feed.ingest(&mut source, &mut receiver, Duration::ZERO)
                .await
                .unwrap();
        }
        feed.release_source(&source).await.unwrap();
        let events = feed.read(None, 100).await.unwrap().events().to_vec();
        let policy = PolicyStore::default();
        let initial = if bot {
            grant()
        } else {
            Grant::new(identity(false), Budget::new("daily").unwrap())
                .unwrap()
                .allow(Action::ReadUpdates)
                .peers(PeerScope::only([peer(7)]))
        };
        policy
            .replace_mtproto(&actor(), "personal", initial)
            .unwrap();
        let executor = Executor::new(policy.clone(), options).unwrap();
        executor
            .register_profile(feed.clone(), client.clone())
            .unwrap();
        Self {
            dir,
            journal,
            feed,
            client,
            policy,
            executor,
            control,
            events,
        }
    }
    fn reference(&self) -> tg_agent::mtproto::CallbackReference {
        self.executor
            .observe_callback(&actor(), "personal", &self.events[1])
            .unwrap()
    }
    async fn close(&self) {
        self.executor.close().await.unwrap();
        self.client.close().await.unwrap();
        self.journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn real_intake_issues_only_authorized_primary_peer_references_and_plain_reader_exports_no_authority()
 {
    let f = Fixture::new(ExecutorOptions::default()).await;
    let before = f.control.identities.load(Ordering::SeqCst);
    let page = f.executor.execute(&actor(), &read(), None).await.unwrap();
    let events = page.read_page().unwrap().events();
    assert_eq!(events.len(), 1);
    let reference = events[0].callback_reference().unwrap().to_owned();
    assert!(reference.starts_with("mtcb_") && reference.len() == 37);
    assert_eq!(reference, f.reference().reference());
    let expiry = f.reference().expires_at_ms();
    let again = f.executor.execute(&actor(), &read(), None).await.unwrap();
    assert_eq!(
        again.read_page().unwrap().events()[0].callback_reference(),
        Some(reference.as_str())
    );
    assert_eq!(expiry, f.reference().expires_at_ms());
    let wire = serde_json::to_string(&page).unwrap();
    for secret in [
        "query_id",
        "access_hash",
        "987654321",
        "origin",
        "observed",
        "chat_instance",
    ] {
        assert!(!wire.contains(secret), "{secret}");
    }
    let reader = Reader::new(f.feed.clone(), f.policy.clone(), ReadOptions::default()).unwrap();
    let plain = reader.read(&actor(), &read(), None).await.unwrap();
    assert!(
        plain
            .events()
            .iter()
            .all(|e| e.callback_reference().is_none())
    );
    assert_eq!(
        before,
        f.control.identities.load(Ordering::SeqCst),
        "read/observe use no RPC"
    );
    f.policy
        .replace_mtproto(&actor(), "personal", grant().unscoped_updates(true))
        .unwrap();
    let all = f.executor.execute(&actor(), &read(), None).await.unwrap();
    let events = all.read_page().unwrap().events();
    assert_eq!(events.len(), 3); // baseline + peer7 + inline; peer9 remains forbidden
    assert_eq!(
        events
            .iter()
            .filter(|e| e.callback_reference().is_some())
            .count(),
        2
    );
    let inline = f
        .executor
        .observe_callback(&actor(), "personal", &f.events[3])
        .unwrap();
    assert_eq!(inline.peer(), None);
    let result = f
        .executor
        .execute(&actor(), &answer(inline.reference(), "inline"), None)
        .await
        .unwrap();
    assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
    // Observation needs read OR answer authority; possession never supplies either.
    let only = |action| {
        Grant::new(identity(true), Budget::new("daily").unwrap())
            .unwrap()
            .allow(action)
            .peers(PeerScope::only([peer(7)]))
    };
    f.policy
        .replace_mtproto(&actor(), "personal", only(Action::ReadUpdates))
        .unwrap();
    let readable = f.reference();
    assert_eq!(
        f.executor
            .execute(&actor(), &answer(readable.reference(), "read-only"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.policy
        .replace_mtproto(&actor(), "personal", only(Action::AnswerCallbackQuery))
        .unwrap();
    let actionable = f.reference();
    f.executor
        .execute(
            &actor(),
            &answer(actionable.reference(), "answer-only"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &read(), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.close().await;
}

#[tokio::test]
async fn observation_and_aliases_require_current_owner_bot_profile_and_exact_journal_origin() {
    let f = Fixture::new(ExecutorOptions::default()).await;
    let reference = f.reference();
    f.policy
        .replace_mtproto(&bob(), "personal", grant())
        .unwrap();
    for (who, id) in [
        (bob(), reference.reference()),
        (actor(), "0"),
        (actor(), "mtcb_unknown"),
    ] {
        assert_eq!(
            f.executor
                .execute(&who, &answer(id, "missing"), None)
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::ReferenceUnavailable
        );
    }
    assert_eq!(
        f.executor
            .observe_callback(&actor(), "personal", &f.events[2])
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(
        f.executor
            .observe_callback(&actor(), "personal", &f.events[3])
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    let other = Journal::open(f.dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let other_feed = Feed::open(
        other.clone(),
        "personal",
        identity(true),
        Options::default(),
    )
    .await
    .unwrap();
    let foreign = other_feed.read(None, 100).await.unwrap();
    assert_eq!(
        f.executor
            .observe_callback(&actor(), "personal", &foreign.events()[1])
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    other.close().await.unwrap();
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().peers(PeerScope::only([peer(99)])),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(
                &actor(),
                &answer(reference.reference(), "actor-is-not-peer"),
                None
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.close().await;
    let user = Fixture::with_kind(ExecutorOptions::default(), false).await;
    assert_eq!(
        user.executor
            .observe_callback(&actor(), "personal", &user.events[1])
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    let invalid = Grant::new(identity(false), Budget::new("daily").unwrap())
        .unwrap()
        .allow(Action::AnswerCallbackQuery);
    assert_eq!(
        user.policy
            .replace_mtproto(&actor(), "personal", invalid)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    let page = user
        .executor
        .execute(&actor(), &read(), None)
        .await
        .unwrap();
    assert!(
        page.read_page()
            .unwrap()
            .events()
            .iter()
            .all(|e| e.callback_reference().is_none())
    );
    user.close().await;
}

#[tokio::test]
async fn trusted_event_alias_supports_exact_preconfigured_approval_without_refreshing_time() {
    let f = Fixture::new(ExecutorOptions::default()).await;
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().require_approval(Action::AnswerCallbackQuery),
        )
        .unwrap();
    let alias = format!("mtcb_{}", "a".repeat(32));
    let reference = f
        .executor
        .observe_callback_as(&actor(), "personal", &f.events[1], &alias)
        .unwrap();
    let expiry = reference.expires_at_ms();
    let exact = answer(&alias, "host-configured");
    let approval = f
        .executor
        .approve(&actor(), &exact, (now() + 1000).min(expiry - 100))
        .await
        .unwrap();
    let again = f
        .executor
        .observe_callback_as(&actor(), "personal", &f.events[1], &alias)
        .unwrap();
    assert_eq!(again.reference(), alias);
    assert_eq!(again.expires_at_ms(), expiry);
    for _ in 0..2 {
        let result = f
            .executor
            .execute(&actor(), &exact, Some(&approval))
            .await
            .unwrap();
        assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
    }
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    let other = format!("mtcb_{}", "b".repeat(32));
    assert_eq!(
        f.executor
            .observe_callback_as(&actor(), "personal", &f.events[1], &other)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    f.close().await;
}

#[tokio::test]
async fn confirmed_ack_is_cached_and_canonical_query_is_fenced_across_aliases_keys_and_principals()
{
    let f = Fixture::new(ExecutorOptions::default()).await;
    let reference = f.reference();
    let req = answer(reference.reference(), "answer");
    let result = f.executor.execute(&actor(), &req, None).await.unwrap();
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
    assert!(result.operation().unwrap().message().is_none());
    let identities = f.control.identities.load(Ordering::SeqCst);
    f.executor.execute(&actor(), &req, None).await.unwrap();
    f.executor
        .execute(&actor(), &lookup("answer"), None)
        .await
        .unwrap();
    assert_eq!(identities, f.control.identities.load(Ordering::SeqCst));
    assert_eq!(
        f.executor
            .execute(&actor(), &answer(reference.reference(), "other-key"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    f.policy
        .replace_mtproto(&bob(), "personal", grant())
        .unwrap();
    let alias = f
        .executor
        .observe_callback(&bob(), "personal", &f.events[1])
        .unwrap();
    assert_ne!(alias.reference(), reference.reference());
    assert_eq!(
        f.executor
            .execute(&bob(), &answer(alias.reference(), "bob-key"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    let wire = serde_json::to_string(&result).unwrap();
    for secret in ["PRIVATE_", "query_id", "reference", "origin"] {
        assert!(!wire.contains(secret));
    }
    f.policy
        .replace_mtproto(&actor(), "personal", grant().peers(PeerScope::only([])))
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("answer"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.close().await;
}

#[tokio::test]
async fn false_refusal_and_timeout_keep_durable_state_and_never_replay_or_open_another_key() {
    for (mode, state) in [
        (1, OperationState::Unknown),
        (2, OperationState::Rejected),
        (3, OperationState::Unknown),
    ] {
        let f = Fixture::new(ExecutorOptions::default()).await;
        let reference = f.reference();
        let req = answer(reference.reference(), "failure");
        f.control.mode.store(mode, Ordering::SeqCst);
        let first = f.executor.execute(&actor(), &req, None).await.unwrap();
        assert_eq!(first.operation().unwrap().state(), state);
        assert_eq!(first.operation().unwrap().acknowledged(), None);
        assert!(first.operation().unwrap().failure().is_some());
        assert!(!serde_json::to_string(&first).unwrap().contains("PRIVATE_"));
        f.control.mode.store(0, Ordering::SeqCst);
        assert_eq!(
            f.executor
                .execute(&actor(), &req, None)
                .await
                .unwrap()
                .operation()
                .unwrap()
                .state(),
            state
        );
        assert_eq!(
            f.executor
                .execute(&actor(), &answer(reference.reference(), "new-key"), None)
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Conflict
        );
        assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
        f.close().await;
    }
}

#[tokio::test]
async fn approval_is_exact_bounded_by_original_reference_and_invalidated_by_replacement() {
    let f = Fixture::new(ExecutorOptions::default()).await;
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().require_approval(Action::AnswerCallbackQuery),
        )
        .unwrap();
    let reference = f.reference();
    let req = answer(reference.reference(), "approved");
    assert_eq!(
        f.executor
            .execute(&actor(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    assert_eq!(
        f.executor
            .approve(&actor(), &req, reference.expires_at_ms() + 1)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    let approval = f
        .executor
        .approve(&actor(), &req, reference.expires_at_ms() - 100)
        .await
        .unwrap();
    let changed = request(
        "mtproto_answer_callback_query",
        json!({"reference":reference.reference(),"text":"changed"}),
        Some("approved"),
    );
    assert_eq!(
        f.executor
            .execute(&actor(), &changed, Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let alternate = Executor::new(f.policy.clone(), ExecutorOptions::default()).unwrap();
    alternate
        .register_profile(f.feed.clone(), f.client.clone())
        .unwrap();
    let alias = alternate
        .observe_callback(&actor(), "personal", &f.events[1])
        .unwrap();
    assert_eq!(
        alternate
            .execute(
                &actor(),
                &answer(alias.reference(), "approved"),
                Some(&approval)
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    alternate.close().await.unwrap();
    f.executor
        .execute(&actor(), &req, Some(&approval))
        .await
        .unwrap();
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().require_approval(Action::AnswerCallbackQuery),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &req, Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    // Lookup rechecks the original scope, without renewing the write approval.
    f.executor
        .execute(&actor(), &lookup("approved"), None)
        .await
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn expired_reference_refuses_cached_answer_but_retained_operation_remains_scoped_and_readable()
 {
    let f = Fixture::new(ExecutorOptions::default().callback_ttl(Duration::from_millis(500))).await;
    let reference = f.reference();
    let req = answer(reference.reference(), "retained");
    f.executor.execute(&actor(), &req, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(
        (reference.expires_at_ms() - now()).max(0) as u64 + 20,
    ))
    .await;
    assert_eq!(
        f.executor
            .execute(&actor(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceExpired
    );
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("retained"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .acknowledged(),
        Some(true)
    );
    assert_eq!(f.executor.prune_callback_references().unwrap(), 1);
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn reference_cap_stops_identity_wait_after_durable_begin_without_callback_dispatch() {
    let f = Fixture::new(ExecutorOptions::default().callback_ttl(Duration::from_millis(400))).await;
    let reference = f.reference();
    f.control.delay_identity.store(true, Ordering::SeqCst);
    let req = answer(reference.reference(), "capped");
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        f.executor.execute(&actor(), &req, None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::ReferenceExpired);
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::Unknown));
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 0);
    let result = f
        .executor
        .execute(&actor(), &lookup("capped"), None)
        .await
        .unwrap();
    assert_eq!(result.operation().unwrap().state(), OperationState::Unknown);
    f.close().await;
}

#[tokio::test]
async fn caller_cancellation_retains_capacity_and_resumable_close_joins_confirmation() {
    let f = Fixture::new(ExecutorOptions::default().max_in_flight(1)).await;
    let req = answer(f.reference().reference(), "cancelled");
    f.control.mode.store(4, Ordering::SeqCst);
    let caller = tokio::spawn({
        let e = f.executor.clone();
        let req = req.clone();
        async move { e.execute(&actor(), &req, None).await }
    });
    tokio::time::timeout(Duration::from_secs(2), f.control.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("cancelled"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    let closer = tokio::spawn({
        let e = f.executor.clone();
        async move { e.close().await }
    });
    tokio::task::yield_now().await;
    closer.abort();
    let _ = closer.await;
    f.control.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), f.executor.close())
        .await
        .unwrap()
        .unwrap();
    let verifier = Executor::new(f.policy.clone(), ExecutorOptions::default()).unwrap();
    verifier
        .register_profile(f.feed.clone(), f.client.clone())
        .unwrap();
    assert_eq!(
        verifier
            .execute(&actor(), &lookup("cancelled"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    verifier.close().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn revocation_while_answer_is_pending_withholds_receipt_and_preserves_confirmed_journal_result()
 {
    let f = Fixture::new(ExecutorOptions::default()).await;
    let req = answer(f.reference().reference(), "revoked");
    f.control.mode.store(4, Ordering::SeqCst);
    let caller = tokio::spawn({
        let e = f.executor.clone();
        async move { e.execute(&actor(), &req, None).await }
    });
    tokio::time::timeout(Duration::from_secs(2), f.control.entered.notified())
        .await
        .unwrap();
    f.policy.revoke(&actor(), "personal").unwrap();
    f.control.release.notify_one();
    let error = caller.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::Denied);
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::Unknown));
    f.policy
        .replace_mtproto(&actor(), "personal", grant())
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("revoked"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .acknowledged(),
        Some(true)
    );
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn restart_requires_explicit_wall_clock_and_semantic_not_sent_recovery_ignores_only_the_new_alias()
 {
    let f = Fixture::new(ExecutorOptions::default()).await;
    let old = f.reference();
    let req = answer(old.reference(), "not-sent");
    // Closed transport is a confirmed NotSent attempt AFTER durable begin.
    f.client.close().await.unwrap();
    let first = f.executor.execute(&actor(), &req, None).await.unwrap();
    assert_eq!(first.operation().unwrap().state(), OperationState::Prepared);
    assert_eq!(
        first.operation().unwrap().failure().unwrap().outcome(),
        "not_sent"
    );
    f.close().await;
    let journal = Journal::open(f.dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        identity(true),
        Options::default(),
    )
    .await
    .unwrap();
    let events = feed.read(None, 100).await.unwrap();
    assert!(events.events()[1].callback_observed_at().is_none());
    let control = Arc::new(Control::default());
    let client = client(
        &f.dir.path().join("restored-session.sqlite"),
        control.clone(),
        true,
    )
    .await;
    let default = Executor::new(f.policy.clone(), ExecutorOptions::default()).unwrap();
    default
        .register_profile(feed.clone(), client.clone())
        .unwrap();
    assert_eq!(
        default
            .observe_callback(&actor(), "personal", &events.events()[1])
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ReferenceUnavailable
    );
    default.close().await.unwrap();
    let restored = Executor::new(
        f.policy.clone(),
        ExecutorOptions::default().callback_clock_policy(CallbackClockPolicy::TrustedWallClock),
    )
    .unwrap();
    restored.register_profile(feed, client.clone()).unwrap();
    let alias = restored
        .observe_callback(&actor(), "personal", &events.events()[1])
        .unwrap();
    assert_ne!(alias.reference(), old.reference());
    assert_eq!(alias.expires_at_ms(), old.expires_at_ms());
    assert_eq!(
        restored
            .execute(&actor(), &answer(alias.reference(), "other-key"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    let changed = request(
        "mtproto_answer_callback_query",
        json!({"reference":alias.reference(),"text":"changed"}),
        Some("not-sent"),
    );
    assert_eq!(
        restored
            .execute(&actor(), &changed, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    let result = restored
        .execute(&actor(), &answer(alias.reference(), "not-sent"), None)
        .await
        .unwrap();
    assert_eq!(result.operation().unwrap().acknowledged(), Some(true));
    assert_eq!(control.writes.load(Ordering::SeqCst), 1);
    restored.close().await.unwrap();
    client.close().await.unwrap();
    journal.close().await.unwrap();
}
