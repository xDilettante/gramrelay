use serde_json::json;
use tg_agent::journal::{
    Completion, Journal, JournalErrorKind, JournalOptions, OperationRequest, OperationState,
    Preparation,
};

fn request(id: &str, text: &str) -> OperationRequest {
    OperationRequest::new(
        "bot-a",
        "alice",
        id,
        "send_message",
        &json!({"chat_id":1,"text":text}),
    )
    .unwrap()
}
fn prepared(result: Preparation) -> tg_agent::journal::Prepared {
    match result {
        Preparation::Ready(lease) => lease,
        Preparation::Existing(_) => panic!("expected executable reservation"),
    }
}

#[tokio::test]
async fn success_is_durable_and_duplicate_calls_never_receive_another_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let operation = request("operation-1", "PRIVATE_PAYLOAD");
    let lease = prepared(journal.prepare(operation.clone()).await.unwrap());
    assert!(
        matches!(journal.prepare(operation.clone()).await.unwrap(), Preparation::Existing(snapshot) if snapshot.state() == OperationState::Prepared)
    );
    let flight = journal.begin(lease).await.unwrap();
    assert!(
        matches!(journal.prepare(operation.clone()).await.unwrap(), Preparation::Existing(snapshot) if snapshot.state() == OperationState::InFlight)
    );
    let snapshot = journal
        .complete(
            flight,
            Completion::succeeded(json!({"message_id":42,"PRIVATE_RESULT":"test"})),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.state(), OperationState::Succeeded);
    assert!(!format!("{snapshot:?} {journal:?} {operation:?}").contains("PRIVATE_"));
    drop(journal);
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(reopened.prepare(operation).await.unwrap(), Preparation::Existing(snapshot) if snapshot.result().unwrap()["message_id"] == 42)
    );
    assert_eq!(
        reopened
            .get("bot-a", "alice", "operation-1")
            .await
            .unwrap()
            .unwrap()
            .state(),
        OperationState::Succeeded
    );
}

#[tokio::test]
async fn canonical_payload_owner_action_and_profile_are_bound_to_the_key() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("operations.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    journal.prepare(request("one", "hello")).await.unwrap();
    let equivalent = OperationRequest::new(
        "bot-a",
        "alice",
        "one",
        "send_message",
        &json!({"text":"hello","chat_id":1}),
    )
    .unwrap();
    assert!(matches!(
        journal.prepare(equivalent).await.unwrap(),
        Preparation::Existing(_)
    ));
    assert_eq!(
        journal
            .prepare(request("one", "changed"))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    let wrong_owner = OperationRequest::new(
        "bot-a",
        "mallory",
        "one",
        "send_message",
        &json!({"chat_id":1,"text":"hello"}),
    )
    .unwrap();
    assert_eq!(
        journal.prepare(wrong_owner).await.unwrap_err().kind(),
        JournalErrorKind::Denied
    );
    assert!(
        journal
            .get("bot-a", "mallory", "one")
            .await
            .unwrap()
            .is_none()
    );
    let changed_action = OperationRequest::new(
        "bot-a",
        "alice",
        "one",
        "send_document",
        &json!({"chat_id":1,"text":"hello"}),
    )
    .unwrap();
    assert_eq!(
        journal.prepare(changed_action).await.unwrap_err().kind(),
        JournalErrorKind::Conflict
    );
    let other_profile = OperationRequest::new(
        "bot-b",
        "alice",
        "one",
        "send_message",
        &json!({"chat_id":1,"text":"hello"}),
    )
    .unwrap();
    assert!(matches!(
        journal.prepare(other_profile).await.unwrap(),
        Preparation::Ready(_)
    ));
}

#[tokio::test]
async fn independent_connections_compete_for_one_reservation_and_versions_fence_stale_handles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let first = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        first.prepare(request("race", "once")),
        second.prepare(request("race", "once"))
    );
    let (owner, lease) = match (a.unwrap(), b.unwrap()) {
        (Preparation::Ready(lease), Preparation::Existing(_)) => (&first, lease),
        (Preparation::Existing(_), Preparation::Ready(lease)) => (&second, lease),
        pair => panic!("multiple/no winners: {pair:?}"),
    };
    let stale = lease.clone();
    let flight = owner.begin(lease).await.unwrap();
    assert_eq!(
        owner.begin(stale).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    let stale = flight.clone();
    owner
        .complete(flight, Completion::succeeded(json!({"message_id":1})))
        .await
        .unwrap();
    assert_eq!(
        owner
            .complete(stale, Completion::succeeded(json!({"message_id":2})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
}

#[tokio::test]
async fn request_bounds_and_storage_errors_are_safe() {
    assert!(OperationRequest::new("", "alice", "op", "send_message", &json!({})).is_err());
    assert!(
        OperationRequest::new(
            "bot",
            "alice",
            "op",
            "send_message",
            &json!({"text":"x".repeat(70_000)})
        )
        .is_err()
    );
    let mut deep = json!(null);
    for _ in 0..70 {
        deep = json!([deep]);
    }
    assert!(OperationRequest::new("bot", "alice", "op", "send_message", &deep).is_err());
    let error = Journal::open("PRIVATE_RELATIVE_PATH", JournalOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), JournalErrorKind::Validation);
    assert!(!format!("{error:?} {error}").contains("PRIVATE_"));
}

#[tokio::test]
async fn leases_from_another_database_are_not_valid_even_when_keys_and_versions_match() {
    let directory = tempfile::tempdir().unwrap();
    let first = Journal::open(directory.path().join("a.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let second = Journal::open(directory.path().join("b.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let a = prepared(first.prepare(request("same", "same")).await.unwrap());
    let b = prepared(second.prepare(request("same", "same")).await.unwrap());
    assert_eq!(
        second.begin(a.clone()).await.unwrap_err().kind(),
        JournalErrorKind::LeaseLost
    );
    let a = first.begin(a).await.unwrap();
    let b = second.begin(b).await.unwrap();
    assert_eq!(
        second
            .complete(a.clone(), Completion::succeeded(json!({})))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    first
        .complete(a, Completion::succeeded(json!({})))
        .await
        .unwrap();
    second
        .complete(b, Completion::succeeded(json!({})))
        .await
        .unwrap();
    first.close().await.unwrap();
    second.close().await.unwrap();
}
