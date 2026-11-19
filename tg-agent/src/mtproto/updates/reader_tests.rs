use super::*;
use crate::{
    executor::{BudgetLimits, ExecutorErrorKind, ReadUpdatesArgs},
    mtproto::{Action, Budget, Call, Grant, PeerScope, PolicyStore, Principal, Request},
};

fn principal() -> Principal {
    Principal::new("alice").unwrap()
}
fn peer(id: i64) -> PeerId {
    PeerId::new(PeerKind::User, id).unwrap()
}
fn request(cursor: Option<String>, limit: u8) -> Request {
    Request::new(
        "personal",
        None,
        Call::ReadUpdates(ReadUpdatesArgs { cursor, limit }),
    )
    .unwrap()
}
fn grant() -> Grant {
    Grant::new(
        ProfileIdentity::mtproto_user(42).unwrap(),
        Budget::new("reads").unwrap(),
    )
    .unwrap()
    .allow(Action::ReadUpdates)
    .peers(PeerScope::only([peer(7)]))
}
struct Fixture {
    _directory: tempfile::TempDir,
    journal: Journal,
    feed: Feed,
    policy: PolicyStore,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("read.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let feed = Feed::open(
            journal.clone(),
            "personal",
            ProfileIdentity::mtproto_user(42).unwrap(),
            Options::default(),
        )
        .await
        .unwrap();
        let policy = PolicyStore::default();
        policy
            .replace_mtproto(&principal(), "personal", grant())
            .unwrap();
        Self {
            _directory: directory,
            journal,
            feed,
            policy,
        }
    }
    async fn append(&self, events: Vec<Event>) {
        let lease = self.feed.claim_source().await.unwrap();
        let encoded = events
            .into_iter()
            .map(|event| project::encode(event, None, &self.feed.options).unwrap())
            .collect();
        self.feed
            .append(
                &lease,
                project::Stamp {
                    source: [1; 32],
                    id: [2; 32],
                    ordinal: 1,
                    kind: "events",
                },
                encoded,
                Instant::now(),
            )
            .await
            .unwrap();
        self.feed.release_source(&lease).await.unwrap();
    }
    fn reader(&self) -> Reader {
        Reader::new(
            self.feed.clone(),
            self.policy.clone(),
            ReadOptions::default(),
        )
        .unwrap()
    }
}
fn message(primary: i64, sender: i64, text: &str) -> Event {
    Event::Message {
        peer: peer(primary),
        message_id: crate::executor::MessageId::new(1).unwrap(),
        sender: Some(peer(sender)),
        date: 1,
        text: text.into(),
        outgoing: false,
        edited: false,
        has_media: false,
    }
}

#[tokio::test]
async fn reader_filters_primary_peers_and_advances_only_considered_records() {
    let fixture = Fixture::new().await;
    fixture
        .append(vec![
            message(9, 7, "FORBIDDEN"),
            message(7, 9, "visible"),
            Event::Unsupported { constructor: 1 },
            message(7, 9, "second"),
        ])
        .await;
    let reader = fixture.reader();
    let first = reader
        .read(&principal(), &request(None, 1), None)
        .await
        .unwrap();
    assert_eq!(first.events().len(), 1);
    assert!(matches!(first.events()[0].event(),Event::Message{text,..} if text=="visible"));
    assert!(first.has_more());
    let second = reader
        .read(
            &principal(),
            &request(Some(first.next_cursor().to_string()), 1),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(second.events()[0].event(),Event::Message{text,..} if text=="second"));
    assert!(!second.has_more());
    let output = serde_json::to_string(&first).unwrap();
    assert!(!output.contains("FORBIDDEN"));
    assert!(output.contains("\"untrusted\":true"));
    assert_eq!(first.events()[0].scope(), ReadScope::Peer(peer(7)));
    assert!(!format!("{first:?} {reader:?}").contains("visible"));
    fixture.journal.close().await.unwrap();
}

async fn usage(journal: &Journal) -> (i64, i64) {
    journal.run(|c,_,_| c.query_row("SELECT COALESCE(SUM(invocations),0),COALESCE(SUM(cost_units),0) FROM executor_budgets",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|storage())).await.unwrap()
}

#[tokio::test]
async fn reader_requires_explicit_unscoped_authority_and_never_exports_callback_metadata() {
    let fixture = Fixture::new().await;
    let lease = fixture.feed.claim_source().await.unwrap();
    let events = vec![
        (Event::Baseline, None),
        (
            Event::Callback {
                peer: None,
                message_id: None,
                actor: peer(7),
                data_base64: Some("aGk=".into()),
                game: None,
            },
            Some(987654321),
        ),
        (
            Event::Deleted {
                peer: None,
                message_ids: vec![],
            },
            None,
        ),
        (
            Event::Gap {
                peer: Some(peer(9)),
                reason: "channel loss".into(),
            },
            None,
        ),
        (Event::Unsupported { constructor: 5 }, None),
    ];
    let encoded = events
        .into_iter()
        .map(|(e, q)| project::encode(e, q, &fixture.feed.options).unwrap())
        .collect();
    fixture
        .feed
        .append(
            &lease,
            project::Stamp {
                source: [1; 32],
                id: [2; 32],
                ordinal: 1,
                kind: "events",
            },
            encoded,
            Instant::now(),
        )
        .await
        .unwrap();
    fixture.feed.release_source(&lease).await.unwrap();
    let reader = fixture.reader();
    let denied = reader
        .read(&principal(), &request(None, 20), None)
        .await
        .unwrap();
    assert!(denied.events().is_empty());
    assert!(!denied.has_more());
    fixture
        .policy
        .replace_mtproto(&principal(), "personal", grant().unscoped_updates(true))
        .unwrap();
    let page = reader
        .read(&principal(), &request(None, 20), None)
        .await
        .unwrap();
    assert_eq!(
        page.events().len(),
        4,
        "unscoped authority cannot disclose the explicitly forbidden channel gap"
    );
    assert!(
        page.events()
            .iter()
            .all(|e| e.scope() == ReadScope::Unscoped)
    );
    let output = serde_json::to_string(&page).unwrap();
    for secret in [
        "987654321",
        "callback_query",
        "callback_received",
        "origin",
        "reference",
    ] {
        assert!(!output.contains(secret), "{secret}");
    }
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_preflight_rejects_foreign_protocol_account_action_and_cursor_without_charging() {
    let fixture = Fixture::new().await;
    fixture.append(vec![message(7, 9, "visible")]).await;
    let reader = fixture.reader();
    let wrong_profile = Request::new(
        "other",
        None,
        Call::ReadUpdates(ReadUpdatesArgs {
            cursor: None,
            limit: 1,
        }),
    )
    .unwrap();
    assert_eq!(
        reader
            .read(&principal(), &wrong_profile, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    let other_action =
        Request::new("personal", None, Call::GetMe(crate::executor::EmptyArgs {})).unwrap();
    assert_eq!(
        reader
            .read(&principal(), &other_action, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::UnsupportedAction
    );
    for cursor in [
        "secret malformed".into(),
        Cursor {
            profile: "personal".into(),
            sequence: 2,
        }
        .to_string(),
        Cursor {
            profile: "other".into(),
            sequence: 0,
        }
        .to_string(),
    ] {
        assert!(
            reader
                .dry_run(&principal(), &request(Some(cursor), 1), None)
                .await
                .is_err()
        );
    }
    for expected in [
        ProfileIdentity::mtproto_bot(42).unwrap(),
        ProfileIdentity::mtproto_user(43).unwrap(),
    ] {
        fixture
            .policy
            .replace_mtproto(
                &principal(),
                "personal",
                Grant::new(expected, Budget::new("reads").unwrap())
                    .unwrap()
                    .allow(Action::ReadUpdates),
            )
            .unwrap();
        assert_eq!(
            reader
                .read(&principal(), &request(None, 1), None)
                .await
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Denied
        );
    }
    fixture
        .policy
        .replace(
            &principal(),
            "personal",
            crate::executor::Grant::new(42, Budget::new("reads").unwrap())
                .allow(crate::executor::Action::ReadUpdates),
        )
        .unwrap();
    assert_eq!(
        reader
            .read(&principal(), &request(None, 1), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(usage(&fixture.journal).await, (0, 0));
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_scans_at_most_100_records_and_validates_host_limits() {
    let fixture = Fixture::new().await;
    let mut events = (0..101)
        .map(|_| message(9, 7, "FORBIDDEN"))
        .collect::<Vec<_>>();
    events.push(message(7, 9, "visible"));
    fixture.append(events).await;
    for options in [
        ReadOptions::default().max_in_flight(0),
        ReadOptions::default().max_in_flight(1025),
        ReadOptions::default().max_read_bytes(0),
        ReadOptions::default().max_read_bytes(64 * 1024 * 1024 + 1),
    ] {
        assert_eq!(
            Reader::new(fixture.feed.clone(), fixture.policy.clone(), options)
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::InvalidPolicy
        );
    }
    let reader = fixture.reader();
    let first = reader
        .read(&principal(), &request(None, 1), None)
        .await
        .unwrap();
    assert!(first.events().is_empty());
    assert!(first.has_more());
    let second = reader
        .read(
            &principal(),
            &request(Some(first.next_cursor().to_string()), 1),
            None,
        )
        .await
        .unwrap();
    assert_eq!(second.events().len(), 1);
    assert!(!second.has_more());
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_metadata_preflight_avoids_payload_and_expired_cursors_fail_before_charge() {
    let fixture = Fixture::new().await;
    fixture.append(vec![message(7, 9, "PRIVATE_CONTENT")]).await;
    fixture
        .journal
        .run(|c, _, _| {
            c.execute("UPDATE mtproto_updates SET payload_hash=zeroblob(32)", [])
                .map_err(|_| storage())?;
            Ok(())
        })
        .await
        .unwrap();
    let reader = fixture.reader();
    reader
        .dry_run(&principal(), &request(None, 1), None)
        .await
        .unwrap();
    reader
        .approve(&principal(), &request(None, 1), now().unwrap() + 30_000)
        .await
        .unwrap();
    assert_eq!(usage(&fixture.journal).await, (0, 0));
    assert_eq!(
        reader
            .read(&principal(), &request(None, 1), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Storage
    );
    assert_eq!(usage(&fixture.journal).await, (1, 1));
    fixture
        .journal
        .run(|c, _, _| {
            c.execute(
                "UPDATE mtproto_updates SET expires_at_ms=received_at_ms",
                [],
            )
            .map_err(|_| storage())?;
            Ok(())
        })
        .await
        .unwrap();
    let stale = Cursor {
        profile: "personal".into(),
        sequence: 0,
    }
    .to_string();
    assert_eq!(
        reader
            .read(&principal(), &request(Some(stale), 1), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::CursorExpired
    );
    assert_eq!(usage(&fixture.journal).await, (1, 1));
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_approvals_are_bound_to_request_revision_principal_and_expiry() {
    let fixture = Fixture::new().await;
    let reader = fixture.reader();
    fixture
        .policy
        .replace_mtproto(
            &principal(),
            "personal",
            grant().require_approval(Action::ReadUpdates),
        )
        .unwrap();
    let req = request(None, 1);
    assert_eq!(
        reader
            .dry_run(&principal(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = reader
        .approve(&principal(), &req, now().unwrap() + 30_000)
        .await
        .unwrap();
    reader
        .dry_run(&principal(), &req, Some(&approval))
        .await
        .unwrap();
    assert_eq!(
        reader
            .dry_run(&principal(), &request(None, 2), Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    fixture
        .policy
        .replace_mtproto(
            &Principal::new("bob").unwrap(),
            "personal",
            grant().require_approval(Action::ReadUpdates),
        )
        .unwrap();
    assert_eq!(
        reader
            .dry_run(&Principal::new("bob").unwrap(), &req, Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    fixture
        .policy
        .replace_mtproto(
            &principal(),
            "personal",
            grant().require_approval(Action::ReadUpdates),
        )
        .unwrap();
    assert_eq!(
        reader
            .read(&principal(), &req, Some(&approval))
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    fixture
        .policy
        .replace_mtproto(
            &principal(),
            "personal",
            grant().expires_at_ms(now().unwrap() - 1),
        )
        .unwrap();
    assert_eq!(
        reader
            .read(&principal(), &req, None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(usage(&fixture.journal).await, (0, 0));
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_dry_run_is_metadata_only_and_bounded_reads_keep_durable_invocation_and_cost_usage()
{
    let fixture = Fixture::new().await;
    fixture.append(vec![message(7, 9, "visible")]).await;
    let budget = Budget::new("reads")
        .unwrap()
        .window(Duration::from_secs(86400))
        .limits(BudgetLimits {
            invocations: 2,
            cost_units: 6,
            ..BudgetLimits::default()
        });
    let bounded = Grant::new(fixture.feed.identity(), budget)
        .unwrap()
        .allow(Action::ReadUpdates)
        .peers(PeerScope::only([peer(7)]))
        .action_cost(Action::ReadUpdates, 3);
    fixture
        .policy
        .replace_mtproto(&principal(), "personal", bounded.clone())
        .unwrap();
    let reader = Reader::new(
        fixture.feed.clone(),
        fixture.policy.clone(),
        ReadOptions::default().max_read_bytes(1),
    )
    .unwrap();
    reader
        .dry_run(&principal(), &request(None, 1), None)
        .await
        .unwrap();
    assert_eq!(usage(&fixture.journal).await, (0, 0));
    assert_eq!(
        reader
            .read(&principal(), &request(None, 1), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Capacity
    );
    assert_eq!(usage(&fixture.journal).await, (1, 3));
    let reader = fixture.reader();
    reader
        .read(&principal(), &request(None, 1), None)
        .await
        .unwrap();
    assert_eq!(usage(&fixture.journal).await, (2, 6));
    let path = fixture._directory.path().join("read.sqlite");
    fixture.journal.close().await.unwrap();
    let journal = Journal::open(path, JournalOptions::default())
        .await
        .unwrap();
    let feed = Feed::open(
        journal.clone(),
        "personal",
        fixture.feed.identity(),
        Options::default(),
    )
    .await
    .unwrap();
    let policy = PolicyStore::default();
    policy
        .replace_mtproto(&principal(), "personal", bounded)
        .unwrap();
    let reader = Reader::new(feed, policy, ReadOptions::default()).unwrap();
    assert_eq!(
        reader
            .read(&principal(), &request(None, 1), None)
            .await
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::QuotaExceeded
    );
    assert_eq!(usage(&journal).await, (2, 6));
    journal.close().await.unwrap();
}

// Stage real native jobs without timing sleeps or test hooks in production code.
async fn pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await
}
struct Hold {
    release: std::sync::mpsc::Sender<()>,
    job: tokio::task::JoinHandle<()>,
}
async fn hold(journal: &Journal) -> Hold {
    let (release, wait) = std::sync::mpsc::channel();
    let (started, ready) = tokio::sync::oneshot::channel();
    let journal = journal.clone();
    let job = tokio::spawn(async move {
        journal
            .run(move |_, _, _| {
                let _ = started.send(());
                wait.recv_timeout(Duration::from_secs(10))
                    .map_err(|_| storage())?;
                Ok(())
            })
            .await
            .unwrap();
    });
    ready.await.unwrap();
    Hold { release, job }
}
impl Hold {
    async fn finish(self) {
        let _ = self.release.send(());
        self.job.await.unwrap();
    }
}
async fn advance<F: std::future::Future>(journal: &Journal, future: std::pin::Pin<&mut F>) {
    let held = hold(journal).await;
    pending(future).await;
    held.finish().await;
    journal.run(|_, _, _| Ok(())).await.unwrap();
}

#[tokio::test]
async fn reader_rechecks_native_quota_admission_and_charges_cancelled_queued_work() {
    let fixture = Fixture::new().await;
    let reader = fixture.reader();
    let req = request(None, 1);
    let actor = principal();
    let mut future = Box::pin(reader.read(&actor, &req, None));
    let held = hold(&fixture.journal).await;
    pending(future.as_mut()).await; // complete read job queued behind native hold
    fixture.policy.revoke(&actor, "personal").unwrap();
    held.finish().await;
    assert_eq!(future.await.unwrap_err().kind(), ExecutorErrorKind::Denied);
    assert_eq!(usage(&fixture.journal).await, (0, 0));
    fixture
        .policy
        .replace_mtproto(&actor, "personal", grant())
        .unwrap();
    let mut future = Box::pin(reader.read(&actor, &req, None));
    let held = hold(&fixture.journal).await;
    pending(future.as_mut()).await;
    drop(future); // native charge remains owned even though caller cancelled
    held.finish().await;
    assert_eq!(usage(&fixture.journal).await, (1, 1));
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_read_keeps_concurrency_slot_until_owned_native_work_finishes() {
    let fixture = Fixture::new().await;
    let reader = Reader::new(
        fixture.feed.clone(),
        fixture.policy.clone(),
        ReadOptions::default().max_in_flight(1),
    )
    .unwrap();
    let actor = principal();
    let req = request(None, 1);
    let held = hold(&fixture.journal).await;
    let mut future = Box::pin(reader.read(&actor, &req, None));
    pending(future.as_mut()).await;
    drop(future);
    let mut another = Box::pin(reader.read(&actor, &req, None));
    let result = std::future::poll_fn(|cx| std::task::Poll::Ready(another.as_mut().poll(cx))).await;
    assert!(
        matches!(result,std::task::Poll::Ready(Err(e)) if e.kind()==ExecutorErrorKind::Capacity),
        "cancelled caller must not release the still-owned native slot"
    );
    held.finish().await;
    fixture.journal.run(|_, _, _| Ok(())).await.unwrap();
    reader.read(&actor, &req, None).await.unwrap();
    fixture.journal.close().await.unwrap();
}

#[tokio::test]
async fn reader_refuses_revoked_or_replaced_authority_after_page_load_and_before_disclosure() {
    let fixture = Fixture::new().await;
    fixture.append(vec![message(7, 9, "PRIVATE_CONTENT")]).await;
    let reader = fixture.reader();
    let actor = principal();
    let req = request(None, 1);
    let mut future = Box::pin(reader.read(&actor, &req, None));
    advance(&fixture.journal, future.as_mut()).await; // page buffered after owned job
    fixture.policy.revoke(&actor, "personal").unwrap();
    assert_eq!(future.await.unwrap_err().kind(), ExecutorErrorKind::Denied);
    fixture
        .policy
        .replace_mtproto(&actor, "personal", grant())
        .unwrap();
    let (auth, _) = reader.preflight(&actor, &req, None).await.unwrap();
    let page = fixture.feed.read(None, 100).await.unwrap();
    let (checked, current) = auth.checked(now().unwrap()).unwrap();
    let projected = super::select(page, &current, 1);
    fixture
        .policy
        .replace_mtproto(&actor, "personal", grant())
        .unwrap();
    assert_eq!(
        auth.disclose(checked.grant_revision, projected)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    assert_eq!(usage(&fixture.journal).await, (1, 1));
    fixture.journal.close().await.unwrap();
}
