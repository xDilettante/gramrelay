#![cfg(feature = "mtproto-test-kit")]
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    executor::ExecutorErrorKind,
    journal::{Journal, JournalOptions, OperationState},
    mtproto::{
        Action, Budget, Executor, ExecutorOptions, Grant, PeerId, PeerKind, PeerScope, PolicyStore,
        Principal, Request,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, Client, ClientOptions, Session, SessionBinding,
    SessionKey, SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
fn expected() -> AccountIdentity {
    AccountIdentity {
        id: AccountId::new(42).unwrap(),
        kind: AccountKind::User,
    }
}
fn account() -> ProfileIdentity {
    ProfileIdentity::mtproto_user(42).unwrap()
}
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn peer() -> PeerId {
    PeerId::new(PeerKind::User, 7).unwrap()
}
fn grant() -> Grant {
    let mut grant = Grant::new(
        account(),
        Budget::new("quota")
            .unwrap()
            .window(Duration::from_secs(86400)),
    )
    .unwrap()
    .peers(PeerScope::only([peer()]));
    for action in [
        Action::GetMe,
        Action::GetPeer,
        Action::ReadUpdates,
        Action::SendMessage,
        Action::EditMessageText,
        Action::GetOperation,
    ] {
        grant = grant.allow(action);
    }
    grant
}
fn request(tool: &str, args: Value, id: Option<&str>) -> Request {
    let mut v =
        json!({"schema_version":1,"profile":"personal","call":{"tool":tool,"arguments":args}});
    if let Some(id) = id {
        v["operation_id"] = json!(id);
    }
    Request::from_json(&serde_json::to_vec(&v).unwrap()).unwrap()
}
fn send(id: &str) -> Request {
    request(
        "mtproto_send_message",
        json!({"peer":peer(),"text":"PRIVATE_TEXT","disable_notification":true}),
        Some(id),
    )
}
fn lookup(id: &str) -> Request {
    request("mtproto_get_operation", json!({"operation_id":id}), None)
}
fn user(id: i64) -> tl::enums::User {
    let mut bytes = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    let flags = if id == 42 { 1u32 << 10 } else { 1 };
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    if id != 42 {
        bytes.extend_from_slice(&987654321i64.to_le_bytes());
    }
    tl::enums::User::from_bytes(&bytes).unwrap()
}
fn sent(id: i32) -> Vec<u8> {
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id,
        pts: 1,
        pts_count: 1,
        date: 123,
        media: None,
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}
fn edited(id: i32, text: &str) -> Vec<u8> {
    let mut bytes = tl::types::Message::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 7 }).to_bytes());
    bytes.extend_from_slice(&123i32.to_le_bytes());
    bytes.extend_from_slice(&text.to_owned().to_bytes());
    let message = tl::enums::Message::from_bytes(&bytes).unwrap();
    tl::enums::Updates::Updates(tl::types::Updates {
        updates: vec![tl::enums::Update::EditMessage(
            tl::types::UpdateEditMessage {
                message,
                pts: 2,
                pts_count: 1,
            },
        )],
        users: vec![],
        chats: vec![],
        date: 123,
        seq: 2,
    })
    .to_bytes()
}
#[derive(Default)]
struct Control {
    calls: Mutex<Vec<u32>>,
    mode: AtomicUsize,
    target: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Control {
    fn target(&self) -> i64 {
        match self.target.load(Ordering::SeqCst) {
            0 => 7,
            id => id as i64,
        }
    }
    fn writes(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|v| {
                **v == tl::functions::messages::SendMessage::CONSTRUCTOR_ID
                    || **v == tl::functions::messages::EditMessage::CONSTRUCTOR_ID
            })
            .count()
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
fn boundary() {
    use std::io::Write;
    println!("TG_AGENT_ACTION_BOUNDARY");
    std::io::stdout().flush().unwrap();
}
async fn respond(
    script: Arc<Control>,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, test_kit::InvocationError> {
    let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    script.calls.lock().unwrap().push(ctor);
    if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
        if script.mode.load(Ordering::SeqCst) == 6 {
            boundary();
            std::future::pending::<()>().await;
        }
        return Ok(vec![user(42)].to_bytes());
    }
    if ctor == tl::functions::contacts::ResolveUsername::CONSTRUCTOR_ID {
        let target = script.target();
        return Ok(
            tl::enums::contacts::ResolvedPeer::Peer(tl::types::contacts::ResolvedPeer {
                peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: target }),
                users: vec![user(target)],
                chats: vec![],
            })
            .to_bytes(),
        );
    }
    script.entered.notify_one();
    match script.mode.load(Ordering::SeqCst) {
        1 => std::future::pending::<()>().await,
        2 => {
            return Err(test_kit::InvocationError::Rpc(
                tl::types::RpcError {
                    error_code: 400,
                    error_message: "PRIVATE_REMOTE_DESCRIPTION".into(),
                }
                .into(),
            ));
        }
        4 => script.release.notified().await,
        5 => {
            boundary();
            std::future::pending::<()>().await;
        }
        _ => {}
    }
    if ctor == tl::functions::messages::SendMessage::CONSTRUCTOR_ID {
        let send = tl::functions::messages::SendMessage::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(send.message, "PRIVATE_TEXT");
        assert!(send.silent);
        assert!(!send.allow_paid_floodskip);
        assert!(send.allow_paid_stars.is_none());
        assert!(
            matches!(send.peer,tl::enums::InputPeer::User(p) if p.user_id==script.target()&&p.access_hash==987654321)
        );
        Ok(sent(if script.mode.load(Ordering::SeqCst) == 3 {
            -1
        } else {
            3
        }))
    } else {
        assert_eq!(ctor, tl::functions::messages::EditMessage::CONSTRUCTOR_ID);
        let edit = tl::functions::messages::EditMessage::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(edit.id, 3);
        Ok(edited(edit.id, edit.message.as_deref().unwrap()))
    }
}
struct Fixture {
    directory: std::path::PathBuf,
    _cleanup: Option<tempfile::TempDir>,
    journal: Journal,
    feed: Feed,
    client: Client,
    control: Arc<Control>,
    policy: PolicyStore,
    executor: Executor,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut fixture = Self::at(directory.path(), false).await;
        fixture._cleanup = Some(directory);
        fixture
    }
    async fn at(path: &std::path::Path, fast: bool) -> Self {
        let directory = path.to_owned();
        let journal = Journal::open(
            directory.join("agent.sqlite"),
            if fast {
                JournalOptions::default().lease_duration(Duration::from_secs(2))
            } else {
                JournalOptions::default()
            },
        )
        .await
        .unwrap();
        let feed = Feed::open(journal.clone(), "personal", account(), Options::default())
            .await
            .unwrap();
        let session = Session::open(
            directory.join("session.sqlite"),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            if fast {
                SessionOptions::default().lease(Duration::from_secs(2))
            } else {
                SessionOptions::default()
            },
        )
        .await
        .unwrap();
        let control = Arc::new(Control::default());
        let script = control.clone();
        let client = test_kit::client(
            session,
            ClientOptions::default().request_timeout(Duration::from_millis(if fast {
                1000
            } else {
                500
            })),
            move |_, bytes| respond(script.clone(), bytes),
        )
        .unwrap();
        client
            .account(expected())
            .resolve_username("synthetic_target")
            .await
            .unwrap();
        control.calls.lock().unwrap().clear();
        let policy = PolicyStore::default();
        policy
            .replace_mtproto(&actor(), "personal", grant())
            .unwrap();
        let executor = Executor::new(
            policy.clone(),
            if fast {
                ExecutorOptions::default().receipt_margin(Duration::from_millis(100))
            } else {
                ExecutorOptions::default()
            },
        )
        .unwrap();
        executor
            .register_profile(feed.clone(), client.clone())
            .unwrap();
        Self {
            directory,
            _cleanup: None,
            journal,
            feed,
            client,
            control,
            policy,
            executor,
        }
    }
    async fn close(&self) {
        self.executor.close().await.unwrap();
        self.client.close().await.unwrap();
        self.journal.close().await.unwrap();
    }
}

#[tokio::test]
async fn text_actions_commit_scoped_receipts_and_cached_replay_never_dispatches_again() {
    let f = Fixture::new().await;
    f.executor
        .dry_run(&actor(), &send("send"), None)
        .await
        .unwrap();
    assert_eq!(f.control.count(), 0);
    let result = f
        .executor
        .execute(&actor(), &send("send"), None)
        .await
        .unwrap();
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(
        result.operation().unwrap().message().unwrap().peer(),
        peer()
    );
    let before = f.control.count();
    f.executor
        .execute(&actor(), &send("send"), None)
        .await
        .unwrap();
    f.executor
        .execute(&actor(), &lookup("send"), None)
        .await
        .unwrap();
    assert_eq!(
        f.control.count(),
        before,
        "cached/get_operation must not verify identity or dispatch"
    );
    assert_eq!(f.control.writes(), 1);
    let edit = request(
        "mtproto_edit_message_text",
        json!({"peer":peer(),"message_id":"3","text":"edited"}),
        Some("edit"),
    );
    assert_eq!(
        f.executor
            .execute(&actor(), &edit, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    let wire = serde_json::to_string(&result).unwrap();
    for secret in ["PRIVATE_", "access_hash", "987654321"] {
        assert!(!wire.contains(secret));
    }
    assert!(!format!("{result:?} {:?}", f.executor).contains("PRIVATE_"));
    f.close().await;
}

#[tokio::test]
async fn remote_reads_use_allowlisted_metadata_and_exact_current_identity_grants() {
    let f = Fixture::new().await;
    let denied = request(
        "mtproto_get_peer",
        json!({"peer":{"kind":"user","id":"9"}}),
        None,
    );
    assert_eq!(
        f.executor
            .execute(&actor(), &denied, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(f.control.count(), 0);
    let me = f
        .executor
        .execute(&actor(), &request("mtproto_get_me", json!({}), None), None)
        .await
        .unwrap();
    assert_eq!(me.account_info().unwrap().account_id(), "42");
    assert_eq!(me.account_info().unwrap().kind(), "mtproto_user");
    let known = f
        .executor
        .execute(
            &actor(),
            &request("mtproto_get_peer", json!({"peer":peer()}), None),
            None,
        )
        .await
        .unwrap();
    assert!(known.peer_info().unwrap().is_cached());
    assert!(!serde_json::to_string(&known).unwrap().contains("987654321"));
    let before = f.control.count();
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            Grant::new(
                ProfileIdentity::mtproto_bot(42).unwrap(),
                Budget::new("quota").unwrap(),
            )
            .unwrap()
            .allow(Action::GetMe),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &request("mtproto_get_me", json!({}), None), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(f.control.count(), before);
    f.close().await;
}

#[tokio::test]
async fn unsupported_capabilities_duplicate_profiles_and_invalid_lease_budgets_are_local() {
    let f = Fixture::new().await;
    assert_eq!(
        f.executor
            .register_profile(f.feed.clone(), f.client.clone())
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert_eq!(
        Executor::new(
            f.policy.clone(),
            ExecutorOptions::default().max_in_flight(0)
        )
        .unwrap_err()
        .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    let long = Executor::new(
        f.policy.clone(),
        ExecutorOptions::default().receipt_margin(Duration::from_secs(60)),
    )
    .unwrap();
    assert_eq!(
        long.register_profile(f.feed.clone(), f.client.clone())
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    for (req, kind) in [
        (
            request(
                "mtproto_answer_callback_query",
                json!({"reference":"missing"}),
                Some("callback"),
            ),
            ExecutorErrorKind::Denied,
        ),
        (
            request(
                "mtproto_send_document",
                json!({"peer":peer(),"file_handle":"missing"}),
                Some("file"),
            ),
            ExecutorErrorKind::UnsupportedAction,
        ),
    ] {
        assert_eq!(
            f.executor
                .execute(&actor(), &req, None)
                .await
                .unwrap_err()
                .kind(),
            kind
        );
    }
    assert_eq!(f.control.count(), 0);
    f.close().await;
}

#[tokio::test]
async fn rejected_unknown_and_malformed_receipts_never_replay_the_same_key() {
    for (mode, state) in [
        (1, OperationState::Unknown),
        (2, OperationState::Rejected),
        (3, OperationState::Unknown),
    ] {
        let f = Fixture::new().await;
        f.control.mode.store(mode, Ordering::SeqCst);
        let first = f
            .executor
            .execute(&actor(), &send("stable"), None)
            .await
            .unwrap();
        assert_eq!(first.operation().unwrap().state(), state);
        assert_eq!(f.control.writes(), 1);
        f.control.mode.store(0, Ordering::SeqCst);
        let calls = f.control.count();
        assert_eq!(
            f.executor
                .execute(&actor(), &send("stable"), None)
                .await
                .unwrap()
                .operation()
                .unwrap()
                .state(),
            state
        );
        assert_eq!(f.control.count(), calls);
        let output = serde_json::to_string(&first).unwrap();
        assert!(!output.contains("PRIVATE_"));
        f.close().await;
    }
}

#[tokio::test]
async fn scopes_owners_and_approvals_are_rechecked_for_retained_receipts() {
    let f = Fixture::new().await;
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().require_approval(Action::SendMessage),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &send("stable"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let approval = f
        .executor
        .approve(&actor(), &send("stable"), now + 30_000)
        .await
        .unwrap();
    assert_eq!(f.control.count(), 0);
    assert_eq!(
        f.executor
            .execute(&actor(), &send("different"), Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    f.executor
        .execute(&actor(), &send("stable"), Some(&approval))
        .await
        .unwrap();
    f.executor
        .execute(&actor(), &lookup("stable"), None)
        .await
        .unwrap();
    let before = f.control.count();
    let bob = Principal::new("bob").unwrap();
    f.policy.replace_mtproto(&bob, "personal", grant()).unwrap();
    assert_eq!(
        f.executor
            .execute(&bob, &lookup("stable"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    let narrowed = Grant::new(account(), Budget::new("quota").unwrap())
        .unwrap()
        .allow(Action::GetOperation)
        .peers(PeerScope::only([peer()]));
    f.policy
        .replace_mtproto(&actor(), "personal", narrowed)
        .unwrap();
    assert_eq!(
        f.executor
            .dry_run(&actor(), &lookup("stable"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant().require_approval(Action::GetOperation),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("stable"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = f
        .executor
        .approve(&actor(), &lookup("stable"), now + 30_000)
        .await
        .unwrap();
    f.executor
        .execute(&actor(), &lookup("stable"), Some(&approval))
        .await
        .unwrap();
    assert_eq!(f.control.count(), before);
    f.close().await;
}

#[tokio::test]
async fn write_quota_survives_journal_restart_and_cached_responses_do_not_charge_another_attempt() {
    use tg_agent::mtproto::BudgetLimits;
    let f = Fixture::new().await;
    let bounded = Grant::new(
        account(),
        Budget::new("quota")
            .unwrap()
            .window(Duration::from_secs(86400))
            .limits(BudgetLimits {
                write_attempts: 1,
                ..BudgetLimits::default()
            }),
    )
    .unwrap()
    .allow(Action::SendMessage)
    .allow(Action::GetOperation)
    .peers(PeerScope::only([peer()]));
    f.policy
        .replace_mtproto(&actor(), "personal", bounded.clone())
        .unwrap();
    f.executor
        .execute(&actor(), &send("first"), None)
        .await
        .unwrap();
    f.executor
        .execute(&actor(), &send("first"), None)
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &send("second"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(f.control.writes(), 1);
    f.executor.close().await.unwrap();
    f.journal.close().await.unwrap();
    let journal = Journal::open(f.directory.join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(journal.clone(), "personal", account(), Options::default())
        .await
        .unwrap();
    let policy = PolicyStore::default();
    policy
        .replace_mtproto(&actor(), "personal", bounded)
        .unwrap();
    let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
    executor.register_profile(feed, f.client.clone()).unwrap();
    assert_eq!(
        executor
            .execute(&actor(), &send("third"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(f.control.writes(), 1);
    executor.close().await.unwrap();
    f.client.close().await.unwrap();
    journal.close().await.unwrap();
}

async fn poll_pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await
}

#[tokio::test]
async fn caller_cancellation_keeps_job_capacity_and_cancelled_close_can_resume_joining() {
    let f = Fixture::new().await;
    let executor = Executor::new(
        f.policy.clone(),
        ExecutorOptions::default().max_in_flight(1),
    )
    .unwrap();
    executor
        .register_profile(f.feed.clone(), f.client.clone())
        .unwrap();
    f.control.mode.store(4, Ordering::SeqCst);
    let who = actor();
    let req = send("cancelled");
    let mut future = Box::pin(executor.execute(&who, &req, None));
    tokio::select! {_=f.control.entered.notified()=>{},result=&mut future=>panic!("early result: {result:?}")};
    drop(future);
    assert_eq!(
        executor
            .execute(&who, &send("other"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    let mut closing = Box::pin(executor.close());
    tokio::select! {result=&mut closing=>panic!("close returned before SDK work: {result:?}"),_=tokio::time::sleep(Duration::from_millis(20))=>{}};
    drop(closing);
    assert_eq!(
        executor
            .execute(&who, &send("after-close"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Closed
    );
    f.control.release.notify_one();
    executor.close().await.unwrap();
    assert_eq!(
        f.executor
            .execute(&who, &lookup("cancelled"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.writes(), 1);
    f.close().await;
}

#[tokio::test]
async fn buffered_owned_receipt_is_not_disclosed_after_grant_replacement() {
    let f = Fixture::new().await;
    let who = actor();
    let req = send("buffered");
    let mut future = Box::pin(f.executor.execute(&who, &req, None));
    poll_pending(future.as_mut()).await;
    f.executor.close().await.unwrap();
    f.policy.replace_mtproto(&who, "personal", grant()).unwrap();
    let error = future.await.unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::Denied);
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::Unknown));
    assert_eq!(
        f.journal
            .get("personal", who.as_str(), "buffered")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    f.close().await;
}

#[tokio::test]
async fn delegated_reader_preserves_authorization_revision_through_the_job_reply() {
    let f = Fixture::new().await;
    let who = actor();
    let req = request("mtproto_read_updates", json!({"limit":10}), None);
    let mut future = Box::pin(f.executor.execute(&who, &req, None));
    poll_pending(future.as_mut()).await;
    f.executor.close().await.unwrap();
    f.policy.replace_mtproto(&who, "personal", grant()).unwrap();
    assert_eq!(future.await.unwrap_err().kind(), ExecutorErrorKind::Denied);
    f.close().await;
}

#[tokio::test]
async fn revocation_during_admitted_rpc_hides_the_receipt_without_losing_its_result() {
    let f = Fixture::new().await;
    f.control.mode.store(4, Ordering::SeqCst);
    let who = actor();
    let req = send("revoked");
    let mut future = Box::pin(f.executor.execute(&who, &req, None));
    tokio::select! {_=f.control.entered.notified()=>{},result=&mut future=>panic!("early result: {result:?}")};
    f.policy.revoke(&who, "personal").unwrap();
    f.control.release.notify_one();
    assert_eq!(future.await.unwrap_err().kind(), ExecutorErrorKind::Denied);
    f.policy.replace_mtproto(&who, "personal", grant()).unwrap();
    assert_eq!(
        f.executor
            .execute(&who, &lookup("revoked"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.writes(), 1);
    f.close().await;
}

#[tokio::test]
async fn a_known_not_sent_missing_peer_can_resume_the_same_key_after_host_cache_preparation() {
    let f = Fixture::new().await;
    let target = PeerId::new(PeerKind::User, 8).unwrap();
    let authority = Grant::new(account(), Budget::new("quota").unwrap())
        .unwrap()
        .allow(Action::SendMessage)
        .peers(PeerScope::only([target]));
    f.policy
        .replace_mtproto(&actor(), "personal", authority)
        .unwrap();
    let req = request(
        "mtproto_send_message",
        json!({"peer":target,"text":"PRIVATE_TEXT","disable_notification":true}),
        Some("resume"),
    );
    let error = f.executor.execute(&actor(), &req, None).await.unwrap_err();
    assert_eq!(error.kind(), ExecutorErrorKind::NotFound);
    assert_eq!(error.outcome(), Some(tg::EffectOutcome::NotSent));
    assert_eq!(f.control.writes(), 0);
    assert_eq!(
        f.journal
            .get("personal", actor().as_str(), "resume")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Prepared
    );
    f.control.target.store(8, Ordering::SeqCst);
    f.client
        .account(expected())
        .resolve_username("new_trusted_peer")
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &req, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.writes(), 1);
    f.close().await;
}

#[tokio::test]
async fn cached_receipts_survive_closed_backend_and_manual_unscoped_rows_are_never_adopted() {
    use tg_agent::journal::{Completion, OperationRequest, Preparation};
    let f = Fixture::new().await;
    f.executor
        .execute(&actor(), &send("cached"), None)
        .await
        .unwrap();
    let Preparation::Ready(p) = f
        .journal
        .prepare(
            OperationRequest::new(
                "personal",
                actor().as_str(),
                "manual",
                "mtproto_send_message",
                &json!({"arbitrary":"data"}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
    else {
        panic!("ready");
    };
    let flight = f.journal.begin(p).await.unwrap();
    f.journal
        .complete(
            flight,
            Completion::succeeded(json!({"PRIVATE_CACHED":"not a scope"})),
        )
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .dry_run(&actor(), &lookup("manual"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.client.close().await.unwrap();
    let count = f.control.count();
    f.executor
        .execute(&actor(), &send("cached"), None)
        .await
        .unwrap();
    f.executor
        .execute(&actor(), &lookup("cached"), None)
        .await
        .unwrap();
    assert_eq!(f.control.count(), count);
    f.close().await;
}

#[test]
#[ignore = "synthetic subprocess fixture; invoked only by the write crash test"]
fn mtproto_write_crash_child() {
    let directory = std::path::PathBuf::from(std::env::var_os("TG_AGENT_MT_WRITE_DIR").unwrap());
    let stage = std::env::var("TG_AGENT_MT_WRITE_STAGE").unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let f = Fixture::at(&directory, true).await;
            match stage.as_str() {
                "before_begin" => f.control.mode.store(6, Ordering::SeqCst),
                "after_begin" => f.control.mode.store(5, Ordering::SeqCst),
                "after_confirm" => {}
                _ => panic!("stage"),
            }
            let result = f
                .executor
                .execute(&actor(), &send("crash"), None)
                .await
                .unwrap();
            assert_eq!(stage, "after_confirm");
            assert_eq!(
                result.operation().unwrap().state(),
                OperationState::Succeeded
            );
            boundary();
            std::future::pending::<()>().await;
        });
}

#[test]
fn real_process_termination_preserves_scope_and_never_replays_an_unknown_write() {
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::mpsc,
    };
    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for (stage, before, after, writes) in [
        ("before_begin", "prepared", OperationState::Succeeded, 1),
        ("after_begin", "in_flight", OperationState::Unknown, 0),
        ("after_confirm", "succeeded", OperationState::Succeeded, 0),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Guard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "mtproto_write_crash_child",
                    "--nocapture",
                ])
                .env("TG_AGENT_MT_WRITE_DIR", dir.path())
                .env("TG_AGENT_MT_WRITE_STAGE", stage)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (signal, wait) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let reached = BufReader::new(stdout)
                .lines()
                .any(|line| line.is_ok_and(|v| v.contains("TG_AGENT_ACTION_BOUNDARY")));
            let _ = signal.send(reached);
        });
        let reached = wait.recv_timeout(Duration::from_secs(10));
        let alive = child.0.try_wait().unwrap().is_none();
        let killed = child.0.kill();
        child.0.wait().unwrap();
        reader.join().unwrap();
        assert_eq!(reached, Ok(true), "child did not reach {stage}");
        assert!(
            alive && killed.is_ok(),
            "parent must actually terminate the live child at {stage}"
        );
        let c = rusqlite::Connection::open(dir.path().join("agent.sqlite")).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT state FROM operations WHERE operation_id='crash'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            before
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM mtproto_operation_scopes", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
        drop(c);
        std::thread::sleep(Duration::from_millis(2200));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let f = Fixture::at(dir.path(), true).await;
                let result = f
                    .executor
                    .execute(&actor(), &send("crash"), None)
                    .await
                    .unwrap();
                assert_eq!(result.operation().unwrap().state(), after, "{stage}");
                assert_eq!(f.control.writes(), writes, "{stage}");
                f.close().await;
            });
    }
}
