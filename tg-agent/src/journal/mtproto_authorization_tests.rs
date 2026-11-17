use super::*;
use crate::executor::{Budget, BudgetLimits};
fn scope() -> Scope {
    Scope {
        account: ProfileIdentity::mtproto_user(42).unwrap(),
        action: Action::SendMessage,
        peer: Some(PeerId::new(PeerKind::User, 7).unwrap()),
        file: None,
        reference_hash: None,
    }
}
fn request(id: &str, owner: &str, action: Action) -> OperationRequest {
    OperationRequest::new(
        "personal",
        owner,
        id,
        action.name(),
        &serde_json::json!({"synthetic":"intent"}),
    )
    .unwrap()
}
fn ready(value: Preparation) -> Prepared {
    let Preparation::Ready(p) = value else {
        panic!("ready");
    };
    p
}
fn budget(owner: &str, attempts: u64) -> BudgetContext {
    BudgetContext {
        profile: "personal".into(),
        principal: owner.into(),
        budget: Budget::new("shared")
            .unwrap()
            .window(Duration::from_secs(86400))
            .limits(BudgetLimits {
                write_attempts: attempts,
                ..BudgetLimits::default()
            }),
        cost: 3,
    }
}
async fn setup(account: ProfileIdentity) -> (tempfile::TempDir, Journal) {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("scope.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    journal.bind_profile("personal", account).await.unwrap();
    (dir, journal)
}

#[tokio::test]
async fn mtproto_prepare_refuses_foreign_accounts_owners_changed_scope_and_unscoped_legacy_rows() {
    let (_dir, journal) = setup(scope().account).await;
    let s = scope();
    let p = ready(
        journal
            .prepare_mtproto(request("key", "alice", s.action), s.clone())
            .await
            .unwrap(),
    );
    assert_eq!(
        journal
            .prepare_mtproto(request("key", "bob", s.action), s.clone())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    let mut changed = s.clone();
    changed.peer = Some(PeerId::new(PeerKind::Channel, 7).unwrap());
    assert_eq!(
        journal
            .prepare_mtproto(request("key", "alice", s.action), changed)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    let mut changed = s.clone();
    changed.account = ProfileIdentity::mtproto_bot(42).unwrap();
    assert_eq!(
        journal
            .prepare_mtproto(request("foreign", "alice", s.action), changed)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert!(
        journal
            .get("personal", "alice", "foreign")
            .await
            .unwrap()
            .is_none()
    );
    journal
        .prepare(request("legacy", "alice", s.action))
        .await
        .unwrap();
    assert_eq!(
        journal
            .mtproto_operation_scope("personal", "alice", "legacy")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert_eq!(
        journal
            .prepare_mtproto(request("legacy", "alice", s.action), s.clone())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    journal.release_prepared(p).await.unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn mtproto_begin_charges_and_marks_admission_atomically_across_workers() {
    let (dir, a) = setup(scope().account).await;
    let b = Journal::open(dir.path().join("scope.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let pa = ready(
        a.prepare_mtproto(request("a", "alice", scope().action), scope())
            .await
            .unwrap(),
    );
    let pb = ready(
        b.prepare_mtproto(request("b", "alice", scope().action), scope())
            .await
            .unwrap(),
    );
    let (ra, rb) = tokio::join!(
        a.begin_mtproto(pa, scope(), |_| Ok(budget("alice", 1))),
        b.begin_mtproto(pb, scope(), |_| Ok(budget("alice", 1)))
    );
    assert_eq!(usize::from(ra.is_ok()) + usize::from(rb.is_ok()), 1);
    let error = if let Err(e) = ra { e } else { rb.unwrap_err() };
    assert_eq!(error.kind(), JournalErrorKind::QuotaExceeded);
    a.run(|c, _, _| {
        let (attempts, cost): (i64, i64) = c
            .query_row(
                "SELECT write_attempts,cost_units FROM executor_budgets",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| storage())?;
        assert_eq!((attempts, cost), (1, 3));
        let admitted: i64 = c
            .query_row(
                "SELECT SUM(admitted) FROM mtproto_operation_scopes",
                [],
                |r| r.get(0),
            )
            .map_err(|_| storage())?;
        assert_eq!(admitted, 1);
        let prepared: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE state='prepared'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| storage())?;
        assert_eq!(
            prepared, 1,
            "failed begin must roll back the in_flight transition"
        );
        Ok(())
    })
    .await
    .unwrap();
    a.close().await.unwrap();
    b.close().await.unwrap();
}

#[tokio::test]
async fn mtproto_callback_admission_is_global_to_the_query_after_not_sent_and_unknown() {
    let mut s = scope();
    s.account = ProfileIdentity::mtproto_bot(42).unwrap();
    s.action = Action::AnswerCallbackQuery;
    s.peer = None;
    s.reference_hash = Some([9; 32]);
    let (_dir, journal) = setup(s.account).await;
    let pa = ready(
        journal
            .prepare_mtproto(request("a", "alice", s.action), s.clone())
            .await
            .unwrap(),
    );
    let pb = ready(
        journal
            .prepare_mtproto(request("b", "bob", s.action), s.clone())
            .await
            .unwrap(),
    );
    let flight = journal
        .begin_mtproto(pa, s.clone(), |_| Ok(budget("alice", 10)))
        .await
        .unwrap();
    journal
        .complete_fenced(flight, Completion::executor_not_started())
        .await
        .unwrap();
    assert_eq!(
        journal
            .begin_mtproto(pb, s.clone(), |_| Ok(budget("bob", 10)))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    let pa = ready(
        journal
            .prepare_mtproto(request("a", "alice", s.action), s.clone())
            .await
            .unwrap(),
    );
    let flight = journal
        .begin_mtproto(pa, s.clone(), |_| Ok(budget("alice", 10)))
        .await
        .unwrap();
    journal
        .complete_fenced(flight, Completion::executor_uncertain())
        .await
        .unwrap();
    assert!(
        matches!(journal.prepare_mtproto(request("a","alice",s.action),s.clone()).await.unwrap(),Preparation::Existing(o) if o.state()==OperationState::Unknown)
    );
    assert_eq!(
        journal
            .prepare_mtproto(request("c", "bob", s.action), s)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn mtproto_scope_validation_preserves_exact_file_metadata_and_rejects_user_callbacks() {
    let (_dir, journal) = setup(scope().account).await;
    let mut s = scope();
    s.action = Action::SendDocument;
    assert_eq!(
        journal
            .prepare_mtproto(request("invalid", "alice", s.action), s.clone())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    s.file = Some(FileScope {
        handle: "file".into(),
        sha256: [1; 32],
        bytes: 7,
    });
    let prepared = ready(
        journal
            .prepare_mtproto(request("doc", "alice", s.action), s.clone())
            .await
            .unwrap(),
    );
    let mut changed = s.clone();
    changed.file.as_mut().unwrap().sha256 = [2; 32];
    assert_eq!(
        journal
            .prepare_mtproto(request("doc", "alice", s.action), changed)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    journal
        .begin_mtproto(prepared, s, |_| Ok(budget("alice", 10)))
        .await
        .unwrap();
    journal
        .run(|c, _, _| {
            let bytes: i64 = c
                .query_row("SELECT upload_bytes FROM executor_budgets", [], |r| {
                    r.get(0)
                })
                .map_err(|_| storage())?;
            assert_eq!(bytes, 7);
            Ok(())
        })
        .await
        .unwrap();
    let mut callback = scope();
    callback.action = Action::AnswerCallbackQuery;
    callback.reference_hash = Some([1; 32]);
    assert_eq!(
        journal
            .prepare_mtproto(request("callback", "alice", callback.action), callback)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Validation
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn mtproto_metadata_load_is_bounded_and_rechecks_durable_account_before_disclosure() {
    let (_dir, journal) = setup(scope().account).await;
    journal
        .prepare_mtproto(request("key", "alice", scope().action), scope())
        .await
        .unwrap();
    journal
        .run(|c, _, _| {
            c.execute("UPDATE mtproto_operation_scopes SET peer_id=?1", [i64::MAX])
                .map_err(|_| storage())?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        journal
            .mtproto_operation_scope("personal", "alice", "key")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Storage
    );
    journal
        .run(|c, _, _| {
            c.execute("UPDATE mtproto_operation_scopes SET peer_id=7", [])
                .map_err(|_| storage())?;
            c.pragma_update(None, "foreign_keys", false)
                .map_err(|_| storage())?;
            c.execute("UPDATE account_profiles SET kind='mtproto_bot'", [])
                .map_err(|_| storage())?;
            c.pragma_update(None, "foreign_keys", true)
                .map_err(|_| storage())?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        journal
            .get_mtproto_scoped("personal", "alice", "key")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    journal.close().await.unwrap();
}
