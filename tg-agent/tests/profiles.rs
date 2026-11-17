use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions, OperationRequest},
    profiles::{ProfileIdentity, ProfileKind},
    updates::{UpdateFeed, UpdateOptions},
};

#[test]
fn account_kinds_are_distinct_and_diagnostics_hide_ids() {
    let identities = [
        ProfileIdentity::bot_api(9007199254740993).unwrap(),
        ProfileIdentity::mtproto_bot(9007199254740993).unwrap(),
        ProfileIdentity::mtproto_user(9007199254740993).unwrap(),
    ];
    for (index, identity) in identities.iter().enumerate() {
        assert_eq!(identity.account_id(), 9007199254740993);
        assert!(!format!("{identity:?}").contains("9007199254740993"));
        for other in &identities[index + 1..] {
            assert_ne!(identity, other);
        }
    }
    assert_eq!(identities[2].kind(), ProfileKind::MtprotoUser);
    assert!(ProfileIdentity::bot_api(0).is_ok());
    for id in [i64::MIN, -1, 0] {
        assert!(ProfileIdentity::mtproto_user(id).is_err());
        assert!(ProfileIdentity::mtproto_bot(id).is_err());
    }
    assert!(ProfileIdentity::bot_api(-1).is_err());
}

#[tokio::test]
async fn exact_bindings_survive_restart_without_alias_or_account_kind_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profiles.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let identities = [
        ("api", ProfileIdentity::bot_api(42).unwrap()),
        ("mt-bot", ProfileIdentity::mtproto_bot(42).unwrap()),
        ("mt-user", ProfileIdentity::mtproto_user(42).unwrap()),
    ];
    for (name, identity) in identities {
        journal.bind_profile(name, identity).await.unwrap();
        journal.bind_profile(name, identity).await.unwrap();
        assert_eq!(
            journal.profile_identity(name).await.unwrap(),
            Some(identity)
        );
        assert_eq!(
            journal
                .bind_profile(format!("{name}-alias"), identity)
                .await
                .unwrap_err()
                .kind(),
            JournalErrorKind::Conflict
        );
    }
    for (name, _) in identities {
        for identity in [
            ProfileIdentity::bot_api(99).unwrap(),
            ProfileIdentity::mtproto_user(99).unwrap(),
        ] {
            assert_eq!(
                journal
                    .bind_profile(name, identity)
                    .await
                    .unwrap_err()
                    .kind(),
                JournalErrorKind::Conflict
            );
        }
    }
    assert!(journal.profile_identity("absent").await.unwrap().is_none());
    journal.close().await.unwrap();
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    for (name, identity) in identities {
        assert_eq!(
            reopened.profile_identity(name).await.unwrap(),
            Some(identity)
        );
        reopened.bind_profile(name, identity).await.unwrap();
    }
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn bot_feed_binding_is_atomic_and_cannot_reinterpret_a_mtproto_profile() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("feeds.sqlite"),
        JournalOptions::default().max_update_feeds(1),
    )
    .await
    .unwrap();
    let user = ProfileIdentity::mtproto_user(42).unwrap();
    journal.bind_profile("user", user).await.unwrap();
    assert_eq!(
        UpdateFeed::open(journal.clone(), "user", 42, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    assert_eq!(journal.profile_identity("user").await.unwrap(), Some(user));
    UpdateFeed::open(journal.clone(), "bot", 42, UpdateOptions::default())
        .await
        .unwrap();
    assert_eq!(
        journal.profile_identity("bot").await.unwrap(),
        Some(ProfileIdentity::bot_api(42).unwrap())
    );
    assert_eq!(
        UpdateFeed::open(journal.clone(), "full", 99, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert!(
        journal.profile_identity("full").await.unwrap().is_none(),
        "failed feed creation must roll back the new binding"
    );
    assert_eq!(
        UpdateFeed::open(
            journal.clone(),
            "bot",
            42,
            UpdateOptions::default().max_event_bytes(1)
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Conflict
    );
    UpdateFeed::open(journal.clone(), "bot", 42, UpdateOptions::default())
        .await
        .unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn legacy_orphan_operations_and_usage_cannot_acquire_a_guessed_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let request = OperationRequest::new(
        "legacy",
        "alice",
        "retained",
        "send_message",
        &serde_json::json!({}),
    )
    .unwrap();
    journal.prepare(request).await.unwrap();
    journal.close().await.unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO executor_budgets VALUES('quota-only','alice','budget',60000,0,3,2,17,4)",
            [],
        )
        .unwrap();
    drop(connection);
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    for name in ["legacy", "quota-only"] {
        for identity in [
            ProfileIdentity::mtproto_user(1).unwrap(),
            ProfileIdentity::bot_api(1).unwrap(),
        ] {
            assert_eq!(
                journal
                    .bind_profile(name, identity)
                    .await
                    .unwrap_err()
                    .kind(),
                JournalErrorKind::Conflict
            );
        }
        assert!(journal.profile_identity(name).await.unwrap().is_none());
        assert_eq!(
            UpdateFeed::open(journal.clone(), name, 1, UpdateOptions::default())
                .await
                .unwrap_err()
                .kind(),
            JournalErrorKind::Conflict
        );
    }
    assert!(
        journal
            .get("legacy", "alice", "retained")
            .await
            .unwrap()
            .is_some()
    );
    journal.close().await.unwrap();
}

#[tokio::test]
async fn capacity_bounds_new_accounts_but_never_breaks_an_existing_binding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_profiles(2))
        .await
        .unwrap();
    let identity = ProfileIdentity::mtproto_user(1).unwrap();
    journal.bind_profile("one", identity).await.unwrap();
    journal
        .bind_profile("two", ProfileIdentity::mtproto_user(2).unwrap())
        .await
        .unwrap();
    journal.close().await.unwrap();
    let journal = Journal::open(&path, JournalOptions::default().max_profiles(1))
        .await
        .unwrap();
    journal.bind_profile("one", identity).await.unwrap();
    assert_eq!(
        journal
            .bind_profile("three", ProfileIdentity::bot_api(3).unwrap())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert_eq!(
        UpdateFeed::open(journal.clone(), "three", 3, UpdateOptions::default())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Capacity
    );
    assert!(journal.profile_identity("three").await.unwrap().is_none());
    journal.close().await.unwrap();
    for bound in [0, 1025] {
        assert_eq!(
            Journal::open(&path, JournalOptions::default().max_profiles(bound))
                .await
                .unwrap_err()
                .kind(),
            JournalErrorKind::Validation
        );
    }
}

#[tokio::test]
async fn independent_workers_serialize_alias_and_conflicting_name_claims() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("race.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let identity = ProfileIdentity::mtproto_user(1).unwrap();
    let (a, b) = tokio::join!(
        first.bind_profile("one", identity),
        second.bind_profile("alias", identity)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let rejected = a.err().or(b.err()).unwrap();
    assert_eq!(rejected.kind(), JournalErrorKind::Conflict);
    let (a, b) = tokio::join!(
        first.bind_profile("shared", ProfileIdentity::mtproto_bot(2).unwrap()),
        second.bind_profile("shared", ProfileIdentity::mtproto_user(2).unwrap())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        a.err().or(b.err()).unwrap().kind(),
        JournalErrorKind::Conflict
    );
    let (a, b) = tokio::join!(
        first.bind_profile("same", ProfileIdentity::bot_api(3).unwrap()),
        second.bind_profile("same", ProfileIdentity::bot_api(3).unwrap())
    );
    a.unwrap();
    b.unwrap();
    first.close().await.unwrap();
    second.close().await.unwrap();
}
