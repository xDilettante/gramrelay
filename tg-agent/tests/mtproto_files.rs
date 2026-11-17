#![cfg(feature = "mtproto-test-kit")]
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    executor::ExecutorErrorKind,
    journal::{Journal, JournalOptions, OperationState},
    mtproto::{
        Action, Budget, BudgetLimits, Executor, ExecutorOptions, FileLimits, Grant, PeerId,
        PeerKind, PeerScope, PolicyStore, Principal, Request,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
};
use tg_mtproto::{
    Client, ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
fn actor() -> Principal {
    Principal::new("alice").unwrap()
}
fn peer() -> PeerId {
    PeerId::new(PeerKind::User, 42).unwrap()
}
fn account() -> ProfileIdentity {
    ProfileIdentity::mtproto_user(42).unwrap()
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn request(tool: &str, args: Value, key: Option<&str>) -> Request {
    let mut value =
        json!({"schema_version":1,"profile":"personal","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        value["operation_id"] = json!(key);
    }
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn document(handle: &str, key: &str) -> Request {
    request(
        "mtproto_send_document",
        json!({"peer":peer(),"file_handle":handle,"caption":"PRIVATE_CAPTION"}),
        Some(key),
    )
}
fn lookup(key: &str) -> Request {
    request("mtproto_get_operation", json!({"operation_id":key}), None)
}
fn grant(handle: &str) -> Grant {
    grant_for(
        handle,
        account(),
        Budget::new("daily")
            .unwrap()
            .window(Duration::from_secs(86400)),
    )
}
fn grant_for(handle: &str, identity: ProfileIdentity, budget: Budget) -> Grant {
    Grant::new(identity, budget)
        .unwrap()
        .allow(Action::SendDocument)
        .allow(Action::GetOperation)
        .peers(PeerScope::only([peer()]))
        .files([handle])
}
fn self_reply(bot: bool) -> Vec<u8> {
    let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&((1u32 << 10) | if bot { 1 << 14 } else { 0 }).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&42i64.to_le_bytes());
    if bot {
        v.extend_from_slice(&0i32.to_le_bytes());
    }
    vec![tl::enums::User::from_bytes(&v).unwrap()].to_bytes()
}
fn sent(size: u64) -> Vec<u8> {
    let mut v = tl::types::MessageMediaDocument::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&tl::types::Document::CONSTRUCTOR_ID.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&123i64.to_le_bytes());
    v.extend_from_slice(&987654321i64.to_le_bytes());
    v.extend_from_slice(&b"PRIVATE_FILE_REFERENCE".to_vec().to_bytes());
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&"application/octet-stream".to_owned().to_bytes());
    v.extend_from_slice(&(size as i64).to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(
        &vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "private.bin".into(),
            },
        )]
        .to_bytes(),
    );
    let media = tl::enums::MessageMedia::from_bytes(&v).unwrap();
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id: 99,
        pts: 1,
        pts_count: 1,
        date: 123,
        media: Some(media),
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}
#[derive(Default)]
struct Control {
    identities: AtomicUsize,
    parts: AtomicUsize,
    sends: AtomicUsize,
    mode: AtomicUsize,
    delay_at: AtomicUsize,
    delay_ms: AtomicUsize,
    bot: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
async fn client(path: &std::path::Path, c: Arc<Control>, max_request: usize) -> Client {
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
        ClientOptions::default()
            .request_timeout(Duration::from_millis(500))
            .max_request_bytes(max_request),
        move |_, bytes| {
            let c = c.clone();
            async move {
                let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
                    let n = c.identities.fetch_add(1, Ordering::SeqCst) + 1;
                    let delayed = c.delay_at.load(Ordering::SeqCst);
                    if delayed > 0 && n >= delayed {
                        c.entered.notify_one();
                        let wait = c.delay_ms.load(Ordering::SeqCst);
                        if wait == 0 {
                            std::future::pending::<()>().await;
                        } else {
                            tokio::time::sleep(Duration::from_millis(wait as u64)).await;
                        }
                    }
                    return Ok(self_reply(c.bot.load(Ordering::SeqCst) != 0));
                }
                if ctor == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID {
                    let part =
                        tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
                    assert_eq!(part.file_part, 0);
                    assert_eq!(part.bytes, b"abc");
                    c.parts.fetch_add(1, Ordering::SeqCst);
                    if c.mode.load(Ordering::SeqCst) == 1 {
                        c.entered.notify_one();
                        std::future::pending::<()>().await;
                    }
                    return Ok(true.to_bytes());
                }
                assert_eq!(ctor, tl::functions::messages::SendMedia::CONSTRUCTOR_ID);
                let send = tl::functions::messages::SendMedia::from_bytes(&bytes[4..]).unwrap();
                assert_eq!(send.message, "PRIVATE_CAPTION");
                assert!(!send.allow_paid_floodskip && send.allow_paid_stars.is_none());
                let tl::enums::InputMedia::UploadedDocument(media) = send.media else {
                    panic!("document")
                };
                assert!(media.force_file);
                assert_eq!(media.mime_type, "application/octet-stream");
                let tl::enums::InputFile::File(file) = media.file else {
                    panic!("small file")
                };
                assert_eq!(file.name, "private.bin");
                assert_eq!(file.parts, 1);
                assert_eq!(file.md5_checksum, "900150983cd24fb0d6963f7d28e17f72");
                c.sends.fetch_add(1, Ordering::SeqCst);
                c.entered.notify_one();
                match c.mode.load(Ordering::SeqCst) {
                    2 => Err(test_kit::InvocationError::Rpc(
                        tl::types::RpcError {
                            error_code: 400,
                            error_message: "PRIVATE_RPC".into(),
                        }
                        .into(),
                    )),
                    3 => Ok(sent(4)),
                    4 => {
                        c.release.notified().await;
                        Ok(sent(3))
                    }
                    _ => Ok(sent(3)),
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
}
impl Fixture {
    async fn new() -> Self {
        Self::with(ExecutorOptions::default(), 4096).await
    }
    async fn with(options: ExecutorOptions, max_request: usize) -> Self {
        Self::with_kind(options, max_request, false).await
    }
    async fn with_kind(options: ExecutorOptions, max_request: usize, bot: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
            .await
            .unwrap();
        let identity = if bot {
            ProfileIdentity::mtproto_bot(42).unwrap()
        } else {
            account()
        };
        let feed = Feed::open(journal.clone(), "personal", identity, Options::default())
            .await
            .unwrap();
        let control = Arc::new(Control::default());
        control.bot.store(usize::from(bot), Ordering::SeqCst);
        let client = client(
            &dir.path().join("session.sqlite"),
            control.clone(),
            max_request,
        )
        .await;
        let policy = PolicyStore::default();
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
        }
    }
    async fn file(&self, expiry: i64) -> tg_agent::mtproto::RegisteredFile {
        let file = self
            .executor
            .register_file(&actor(), "personal", "private.bin", b"abc".to_vec(), expiry)
            .await
            .unwrap();
        self.policy
            .replace_mtproto(
                &actor(),
                "personal",
                grant_for(
                    file.handle(),
                    self.feed.identity(),
                    Budget::new("daily")
                        .unwrap()
                        .window(Duration::from_secs(86400)),
                ),
            )
            .unwrap();
        file
    }
    async fn close(&self) {
        self.executor.close().await.unwrap();
        self.client.close().await.unwrap();
        self.journal.close().await.unwrap();
    }
}
#[tokio::test]
async fn exact_document_upload_has_minimal_scoped_receipt_and_cached_calls_require_no_bytes_or_rpc()
{
    let f = Fixture::new().await;
    let file = f.file(now() + 60_000).await;
    let req = document(file.handle(), "send");
    f.executor.dry_run(&actor(), &req, None).await.unwrap();
    assert_eq!(f.control.identities.load(Ordering::SeqCst), 0);
    let result = f.executor.execute(&actor(), &req, None).await.unwrap();
    let receipt = result.operation().unwrap();
    assert_eq!(receipt.state(), OperationState::Succeeded);
    assert_eq!(receipt.message().unwrap().peer(), peer());
    assert_eq!(receipt.message().unwrap().message_id().get(), 99);
    let wire = serde_json::to_string(&result).unwrap();
    for secret in [
        "PRIVATE_",
        "private.bin",
        "sha256",
        "file_handle",
        "access_hash",
        "987654321",
    ] {
        assert!(!wire.contains(secret), "{secret}");
    }
    assert_eq!(f.control.parts.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.executor
        .discard_file(&actor(), "personal", file.handle())
        .unwrap();
    f.client.close().await.unwrap();
    f.executor.execute(&actor(), &req, None).await.unwrap();
    f.executor
        .execute(&actor(), &lookup("send"), None)
        .await
        .unwrap();
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

fn small_limits() -> FileLimits {
    FileLimits {
        max_file_bytes: 3,
        max_snapshot_bytes: 3,
        max_snapshots: 1,
        max_upload_bytes: 3,
    }
}

#[tokio::test]
async fn host_validation_resident_capacity_and_exact_owner_hash_restoration_share_the_existing_store()
 {
    let f = Fixture::with(ExecutorOptions::default().file_limits(small_limits()), 4096).await;
    for name in ["", "../file", "a:b", "a.", "a ", "a\0b"] {
        assert_eq!(
            f.executor
                .register_file(&actor(), "personal", name, b"abc".to_vec(), now() + 60_000)
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::InvalidInput
        );
    }
    assert_eq!(
        f.executor
            .register_file(&actor(), "personal", "private.bin", vec![], now() + 60_000)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidInput
    );
    let mut excess = Vec::with_capacity(4);
    excess.extend_from_slice(b"abc");
    assert_eq!(
        f.executor
            .register_file(&actor(), "personal", "private.bin", excess, now() + 60_000)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    let file = f.file(now() + 60_000).await;
    assert_eq!(file.byte_size(), 3);
    assert_eq!(
        f.executor
            .register_file(
                &actor(),
                "personal",
                "private.bin",
                b"abc".to_vec(),
                now() + 60_000
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    let bob = Principal::new("bob").unwrap();
    assert!(
        !f.executor
            .discard_file(&bob, "personal", file.handle())
            .unwrap()
    );
    f.executor
        .discard_file(&actor(), "personal", file.handle())
        .unwrap();
    assert_eq!(
        f.executor
            .restore_file(&bob, "personal", file.handle(), b"abc".to_vec())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    assert_eq!(
        f.executor
            .restore_file(&actor(), "personal", file.handle(), b"bad".to_vec())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    let restored = f
        .executor
        .restore_file(&actor(), "personal", file.handle(), b"abc".to_vec())
        .await
        .unwrap();
    assert_eq!(restored.sha256(), file.sha256());
    assert_eq!(restored.expires_at_ms(), file.expires_at_ms());
    assert_eq!(f.control.identities.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
async fn file_registration_grants_no_authority_and_lookup_is_bound_to_current_peer_file_owner_and_profile()
 {
    let f = Fixture::new().await;
    let file = f
        .executor
        .register_file(
            &actor(),
            "personal",
            "private.bin",
            b"abc".to_vec(),
            now() + 60_000,
        )
        .await
        .unwrap();
    let req = document(file.handle(), "denied");
    assert_eq!(
        f.executor
            .execute(&actor(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    for grant in [
        grant(file.handle()).files(Vec::<String>::new()),
        grant(file.handle()).peers(PeerScope::only([])),
    ] {
        f.policy
            .replace_mtproto(&actor(), "personal", grant)
            .unwrap();
        assert_eq!(
            f.executor
                .execute(&actor(), &req, None)
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Denied
        );
    }
    let bob = Principal::new("bob").unwrap();
    f.policy
        .replace_mtproto(&bob, "personal", grant(file.handle()))
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&bob, &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    let other_identity = ProfileIdentity::mtproto_user(43).unwrap();
    let other = Feed::open(
        f.journal.clone(),
        "other",
        other_identity,
        Options::default(),
    )
    .await
    .unwrap();
    f.executor
        .register_profile(other, f.client.clone())
        .unwrap();
    f.policy
        .replace_mtproto(
            &actor(),
            "other",
            grant_for(file.handle(), other_identity, Budget::new("daily").unwrap()),
        )
        .unwrap();
    let mut other_request = serde_json::to_value(&req).unwrap();
    other_request["profile"] = json!("other");
    let other_request = Request::from_json(&serde_json::to_vec(&other_request).unwrap()).unwrap();
    assert_eq!(
        f.executor
            .dry_run(&actor(), &other_request, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    assert_eq!(
        f.executor
            .restore_file(&actor(), "other", file.handle(), b"abc".to_vec())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::NotFound
    );
    assert_eq!(f.control.identities.load(Ordering::SeqCst), 0);
    f.policy
        .replace_mtproto(&actor(), "personal", grant(file.handle()))
        .unwrap();
    f.executor.execute(&actor(), &req, None).await.unwrap();
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant(file.handle()).files(Vec::<String>::new()),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("denied"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    f.close().await;
}

#[tokio::test]
async fn exact_approval_and_revocation_gate_document_disclosure_without_losing_confirmed_effects() {
    let f = Fixture::new().await;
    let file = f.file(now() + 60_000).await;
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant(file.handle()).require_approval(Action::SendDocument),
        )
        .unwrap();
    let req = document(file.handle(), "approved");
    assert_eq!(
        f.executor
            .execute(&actor(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = f
        .executor
        .approve(&actor(), &req, now() + 30_000)
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .execute(
                &actor(),
                &document(file.handle(), "changed-key"),
                Some(&approval)
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    f.policy
        .replace_mtproto(
            &actor(),
            "personal",
            grant(file.handle()).require_approval(Action::SendDocument),
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
    let approval = f
        .executor
        .approve(&actor(), &req, now() + 30_000)
        .await
        .unwrap();
    f.control.mode.store(4, Ordering::SeqCst);
    let caller = tokio::spawn({
        let e = f.executor.clone();
        async move { e.execute(&actor(), &req, Some(&approval)).await }
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
        .replace_mtproto(
            &actor(),
            "personal",
            grant(file.handle()).require_approval(Action::SendDocument),
        )
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("approved"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn expiry_gates_fresh_transmission_but_cached_and_retained_results_need_no_live_snapshot() {
    let f = Fixture::new().await;
    let file = f.file(now() + 400).await;
    let req = document(file.handle(), "retained");
    f.executor.execute(&actor(), &req, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(
        (file.expires_at_ms() - now()).max(0) as u64 + 20,
    ))
    .await;
    f.executor.execute(&actor(), &req, None).await.unwrap();
    assert_eq!(f.executor.prune_files().unwrap(), 1);
    f.executor
        .execute(&actor(), &lookup("retained"), None)
        .await
        .unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &document(file.handle(), "expired"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileExpired
    );
    assert_eq!(
        f.executor
            .restore_file(&actor(), "personal", file.handle(), b"abc".to_vec())
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileExpired
    );
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn rejected_timeout_and_malformed_media_keep_terminal_same_key_state_without_replay() {
    for (mode, state, sends) in [
        (1, OperationState::Unknown, 0),
        (2, OperationState::Rejected, 1),
        (3, OperationState::Unknown, 1),
    ] {
        let f = Fixture::new().await;
        let file = f.file(now() + 60_000).await;
        let req = document(file.handle(), "failure");
        f.control.mode.store(mode, Ordering::SeqCst);
        let result = f.executor.execute(&actor(), &req, None).await.unwrap();
        assert_eq!(result.operation().unwrap().state(), state);
        assert!(result.operation().unwrap().message().is_none());
        assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE_"));
        f.executor
            .discard_file(&actor(), "personal", file.handle())
            .unwrap();
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
        assert_eq!(f.control.parts.load(Ordering::SeqCst), 1);
        assert_eq!(f.control.sends.load(Ordering::SeqCst), sends);
        f.close().await;
    }
}

fn quota_grant(handle: &str, upload_bytes: u64) -> Grant {
    grant_for(
        handle,
        account(),
        Budget::new("daily")
            .unwrap()
            .window(Duration::from_secs(86400))
            .limits(BudgetLimits {
                upload_bytes,
                ..BudgetLimits::default()
            }),
    )
}
async fn reopen(f: &Fixture) -> (Journal, Client, Arc<Control>, Executor) {
    let journal = Journal::open(f.dir.path().join("agent.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(journal.clone(), "personal", account(), Options::default())
        .await
        .unwrap();
    let control = Arc::new(Control::default());
    let client = client(
        &f.dir.path().join("restored-session.sqlite"),
        control.clone(),
        4096,
    )
    .await;
    let executor = Executor::new(f.policy.clone(), ExecutorOptions::default()).unwrap();
    executor.register_profile(feed, client.clone()).unwrap();
    (journal, client, control, executor)
}

#[tokio::test]
async fn upload_quota_is_atomic_and_survives_restart_while_cache_needs_no_restoration() {
    let f = Fixture::new().await;
    let file = f.file(now() + 60_000).await;
    f.policy
        .replace_mtproto(&actor(), "personal", quota_grant(file.handle(), 3))
        .unwrap();
    let first = document(file.handle(), "first");
    let second = document(file.handle(), "second");
    f.executor.execute(&actor(), &first, None).await.unwrap();
    f.executor.execute(&actor(), &first, None).await.unwrap();
    assert_eq!(
        f.executor
            .execute(&actor(), &second, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.close().await;
    let (journal, client, control, executor) = reopen(&f).await;
    executor.execute(&actor(), &first, None).await.unwrap();
    executor
        .execute(&actor(), &lookup("first"), None)
        .await
        .unwrap();
    assert_eq!(control.identities.load(Ordering::SeqCst), 0);
    executor.dry_run(&actor(), &second, None).await.unwrap();
    assert_eq!(
        executor
            .execute(&actor(), &second, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::FileUnavailable
    );
    executor
        .restore_file(&actor(), "personal", file.handle(), b"abc".to_vec())
        .await
        .unwrap();
    assert_eq!(
        executor
            .execute(&actor(), &second, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(control.parts.load(Ordering::SeqCst), 0);
    executor.close().await.unwrap();
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn confirmed_not_sent_same_key_survives_exact_restart_restoration_and_payload_conflicts() {
    let f = Fixture::with(ExecutorOptions::default(), 128).await;
    let file = f.file(now() + 60_000).await;
    let req = document(file.handle(), "not-sent");
    f.policy
        .replace_mtproto(&actor(), "personal", quota_grant(file.handle(), 6))
        .unwrap();
    let first = f.executor.execute(&actor(), &req, None).await.unwrap();
    assert_eq!(first.operation().unwrap().state(), OperationState::Prepared);
    assert_eq!(
        first.operation().unwrap().failure().unwrap().outcome(),
        "not_sent"
    );
    assert_eq!(f.control.parts.load(Ordering::SeqCst), 0);
    f.close().await;
    let (journal, client, control, executor) = reopen(&f).await;
    assert_eq!(
        executor
            .execute(&actor(), &lookup("not-sent"), None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Prepared
    );
    executor
        .restore_file(&actor(), "personal", file.handle(), b"abc".to_vec())
        .await
        .unwrap();
    let changed = request(
        "mtproto_send_document",
        json!({"peer":peer(),"file_handle":file.handle(),"caption":"changed"}),
        Some("not-sent"),
    );
    assert_eq!(
        executor
            .execute(&actor(), &changed, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    assert_eq!(
        executor
            .execute(&actor(), &req, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(
        executor
            .execute(&actor(), &document(file.handle(), "exhausted"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(control.parts.load(Ordering::SeqCst), 1);
    assert_eq!(control.sends.load(Ordering::SeqCst), 1);
    executor.close().await.unwrap();
    client.close().await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn file_expiry_caps_both_cache_identity_and_post_begin_identity_without_late_parts() {
    for after_begin in [false, true] {
        let f = Fixture::new().await;
        let file = f.file(now() + 300).await;
        f.control
            .delay_at
            .store(if after_begin { 2 } else { 1 }, Ordering::SeqCst);
        f.control.delay_ms.store(450, Ordering::SeqCst);
        let req = document(file.handle(), "capped");
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            f.executor.execute(&actor(), &req, None),
        )
        .await
        .unwrap();
        if after_begin {
            assert_eq!(
                result.unwrap().operation().unwrap().state(),
                OperationState::Unknown
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), ExecutorErrorKind::Remote);
            assert_eq!(error.outcome(), Some(tg::EffectOutcome::NotSent));
        }
        assert_eq!(f.control.parts.load(Ordering::SeqCst), 0);
        assert_eq!(f.control.sends.load(Ordering::SeqCst), 0);
        let state = f
            .executor
            .execute(&actor(), &lookup("capped"), None)
            .await
            .unwrap();
        assert_eq!(
            state.operation().unwrap().state(),
            if after_begin {
                OperationState::Unknown
            } else {
                OperationState::Prepared
            }
        );
        f.close().await;
    }
}

#[tokio::test]
async fn bot_document_profile_and_distant_valid_expiry_use_the_same_verified_pipeline() {
    let f = Fixture::with_kind(ExecutorOptions::default(), 4096, true).await;
    let file = f.file(i64::MAX).await;
    let result = f
        .executor
        .execute(&actor(), &document(file.handle(), "bot"), None)
        .await
        .unwrap();
    assert_eq!(
        result.operation().unwrap().state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn two_executors_share_the_durable_claim_and_reject_changed_in_flight_intent() {
    let f = Fixture::new().await;
    let file = f.file(now() + 60_000).await;
    let other = Executor::new(f.policy.clone(), ExecutorOptions::default()).unwrap();
    other
        .register_profile(f.feed.clone(), f.client.clone())
        .unwrap();
    other
        .restore_file(&actor(), "personal", file.handle(), b"abc".to_vec())
        .await
        .unwrap();
    f.control.mode.store(4, Ordering::SeqCst);
    let req = document(file.handle(), "shared");
    let caller = tokio::spawn({
        let e = f.executor.clone();
        let req = req.clone();
        async move { e.execute(&actor(), &req, None).await }
    });
    tokio::time::timeout(Duration::from_secs(2), f.control.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        other
            .execute(&actor(), &req, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::InFlight
    );
    let changed = request(
        "mtproto_send_document",
        json!({"peer":peer(),"file_handle":file.handle(),"caption":"changed"}),
        Some("shared"),
    );
    assert_eq!(
        other
            .execute(&actor(), &changed, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Conflict
    );
    f.control.release.notify_one();
    caller.await.unwrap().unwrap();
    assert_eq!(
        other
            .execute(&actor(), &req, None)
            .await
            .unwrap()
            .operation()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 1);
    other.close().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn cancelled_upload_keeps_memory_and_job_permits_and_cancelled_close_can_resume_confirmation()
{
    let f = Fixture::with(
        ExecutorOptions::default()
            .max_in_flight(1)
            .file_limits(small_limits()),
        4096,
    )
    .await;
    let file = f.file(now() + 60_000).await;
    f.control.mode.store(4, Ordering::SeqCst);
    let req = document(file.handle(), "cancelled");
    let caller = tokio::spawn({
        let e = f.executor.clone();
        async move { e.execute(&actor(), &req, None).await }
    });
    tokio::time::timeout(Duration::from_secs(2), f.control.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    f.executor
        .discard_file(&actor(), "personal", file.handle())
        .unwrap();
    assert_eq!(
        f.executor
            .register_file(
                &actor(),
                "personal",
                "private.bin",
                b"abc".to_vec(),
                now() + 60_000
            )
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    assert_eq!(
        f.executor
            .execute(&actor(), &lookup("cancelled"), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    f.control.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match f
                .executor
                .execute(&actor(), &lookup("cancelled"), None)
                .await
            {
                Ok(result) => {
                    assert_eq!(
                        result.operation().unwrap().state(),
                        OperationState::Succeeded
                    );
                    break;
                }
                Err(error) if error.kind() == ExecutorErrorKind::Capacity => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Err(error) => panic!("unexpected completion: {error:?}"),
            }
        }
    })
    .await
    .unwrap();
    // Source disposal has now released the old snapshot and upload permits.
    let replacement = f.file(now() + 60_000).await;
    let req = document(replacement.handle(), "closing");
    let caller = tokio::spawn({
        let e = f.executor.clone();
        async move { e.execute(&actor(), &req, None).await }
    });
    tokio::time::timeout(Duration::from_secs(2), f.control.entered.notified())
        .await
        .unwrap();
    let closer = tokio::spawn({
        let e = f.executor.clone();
        async move { e.close().await }
    });
    tokio::task::yield_now().await;
    closer.abort();
    let _ = closer.await;
    f.control.release.notify_one();
    caller.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.executor.close())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.control.sends.load(Ordering::SeqCst), 2);
    f.close().await;
}
