use super::{
    Completion, Journal, JournalErrorKind, JournalOptions, OperationRequest, OperationState,
    Preparation,
    authorization::{BudgetContext, Charge, OperationScope, charge},
};
use crate::{
    executor::{Action, Budget, BudgetLimits, ChatId},
    updates::{UpdateFeed, UpdateOptions},
};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::json;
use std::time::Duration;

fn context(limit: u64) -> BudgetContext {
    BudgetContext {
        profile: "primary".into(),
        principal: "alice".into(),
        cost: 1,
        budget: Budget::new("retained-budget")
            .unwrap()
            .limits(BudgetLimits {
                invocations: limit,
                write_attempts: limit,
                ..BudgetLimits::default()
            }),
    }
}
fn request(id: &str) -> OperationRequest {
    OperationRequest::new(
        "primary",
        "alice",
        id,
        "send_message",
        &json!({"chat_id":"-1001","text":"PRIVATE_TEXT"}),
    )
    .unwrap()
}
fn scope() -> OperationScope {
    OperationScope {
        account: crate::profiles::ProfileIdentity::bot_api(123).unwrap(),
        action: Action::SendMessage,
        chat: Some(ChatId::new(-1001).unwrap()),
        file: None,
        reference_hash: None,
    }
}

#[tokio::test]
async fn stored_bot_scopes_cannot_authorize_a_same_number_mtproto_binding() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("binding.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    UpdateFeed::open(journal.clone(), "primary", 123, UpdateOptions::default())
        .await
        .unwrap();
    journal
        .prepare_scoped(request("stored"), scope())
        .await
        .unwrap();
    // Simulate a future bad migration mixing protocol domains. Even then a
    // legacy Bot scope must not establish authority over a MTProto account.
    journal
        .run(|connection, _, _| {
            connection
                .execute(
                    "UPDATE account_profiles SET kind='mtproto_user' WHERE profile='primary'",
                    [],
                )
                .map_err(|_| super::storage())?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        journal
            .operation_scope("primary", "alice", "stored")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert_eq!(
        journal
            .get_scoped("primary", "alice", "stored")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert_eq!(
        journal
            .prepare_scoped(request("fresh"), scope())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    journal.close().await.unwrap();
}
fn ready(value: Preparation) -> super::Prepared {
    match value {
        Preparation::Ready(value) => value,
        _ => panic!("expected reservation"),
    }
}
async fn setup(journal: &Journal) {
    UpdateFeed::open(journal.clone(), "primary", 123, UpdateOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn scoped_rows_are_bound_atomically_and_legacy_rows_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("scopes.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    setup(&journal).await;
    let prepared = ready(
        journal
            .prepare_scoped(request("scoped"), scope())
            .await
            .unwrap(),
    );
    let flight = journal.begin_scoped(prepared, context(10)).await.unwrap();
    journal
        .complete(flight, Completion::succeeded(json!({"message_id":"8"})))
        .await
        .unwrap();
    let (operation, stored) = journal
        .get_scoped("primary", "alice", "scoped")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(operation.state(), OperationState::Succeeded);
    assert_eq!(stored, scope());
    assert!(
        journal
            .get_scoped("primary", "other", "scoped")
            .await
            .unwrap()
            .is_none()
    );
    let mut changed = scope();
    changed.chat = Some(ChatId::new(9).unwrap());
    assert_eq!(
        journal
            .prepare_scoped(request("scoped"), changed)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    journal.prepare(request("legacy")).await.unwrap();
    assert_eq!(
        journal
            .prepare_scoped(request("legacy"), scope())
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    assert_eq!(
        journal
            .get_scoped("primary", "alice", "legacy")
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Denied
    );
    let mut wrong_bot = scope();
    wrong_bot.account = crate::profiles::ProfileIdentity::bot_api(124).unwrap();
    assert!(
        journal
            .prepare_scoped(request("wrong-bot"), wrong_bot)
            .await
            .is_err()
    );
    assert!(
        journal
            .get("primary", "alice", "wrong-bot")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn durable_quota_survives_reopen_new_caps_and_backward_clock_without_refunds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("quota.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    journal.charge_invocation(context(2), 1).await.unwrap();
    journal.close().await.unwrap();
    let reopened = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    reopened.charge_invocation(context(2), 1).await.unwrap();
    assert_eq!(
        reopened
            .charge_invocation(context(2), 1)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::QuotaExceeded
    );
    assert_eq!(
        reopened
            .charge_invocation(context(1), 1)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::QuotaExceeded
    );
    let result = reopened
        .run(move |connection, _, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| super::storage())?;
            // A backward clock cannot select a fresh older bucket.
            charge(
                &transaction,
                &JournalOptions::default(),
                &context(2),
                Charge {
                    invocations: 1,
                    ..Charge::default()
                },
                0,
            )?;
            transaction.commit().map_err(|_| super::storage())
        })
        .await;
    assert_eq!(result.unwrap_err().kind(), JournalErrorKind::QuotaExceeded);
    let altered = BudgetContext {
        budget: Budget::new("retained-budget")
            .unwrap()
            .window(Duration::from_secs(120)),
        ..context(2)
    };
    assert_eq!(
        reopened
            .charge_invocation(altered, 0)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
}

#[tokio::test]
async fn quota_and_in_flight_transition_are_one_transaction_across_connections() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("concurrent.sqlite");
    let one = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    let two = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    setup(&one).await;
    let first = ready(one.prepare_scoped(request("one"), scope()).await.unwrap());
    let second = ready(two.prepare_scoped(request("two"), scope()).await.unwrap());
    let (a, b) = tokio::join!(
        one.begin_scoped(first, context(1)),
        two.begin_scoped(second, context(1))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        a.err().or_else(|| b.err()).unwrap().kind(),
        JournalErrorKind::QuotaExceeded
    );
    let states = [
        one.get("primary", "alice", "one")
            .await
            .unwrap()
            .unwrap()
            .state(),
        one.get("primary", "alice", "two")
            .await
            .unwrap()
            .unwrap()
            .state(),
    ];
    assert_eq!(
        states
            .iter()
            .filter(|v| **v == OperationState::InFlight)
            .count(),
        1
    );
    assert_eq!(
        states
            .iter()
            .filter(|v| **v == OperationState::Prepared)
            .count(),
        1
    );
}

#[tokio::test]
async fn duplicate_stale_begin_does_not_charge_another_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stale.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    setup(&journal).await;
    let prepared = ready(
        journal
            .prepare_scoped(request("one"), scope())
            .await
            .unwrap(),
    );
    let stale = prepared.clone();
    journal.begin_scoped(prepared, context(2)).await.unwrap();
    assert_eq!(
        journal
            .begin_scoped(stale, context(2))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::LeaseLost
    );
    let value: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT write_attempts FROM executor_budgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(value, 1);
}

#[tokio::test]
async fn callback_aliases_bind_only_at_admission_and_losing_claim_rolls_back_quota() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("callback.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    setup(&journal).await;
    let callback_request = |key| {
        OperationRequest::new(
            "primary",
            "alice",
            key,
            "answer_callback_query",
            &json!({"callback_query_id":"PRIVATE_REFERENCE"}),
        )
        .unwrap()
    };
    let callback_scope = OperationScope {
        action: Action::AnswerCallbackQuery,
        reference_hash: Some([7; 32]),
        ..scope()
    };
    let first = ready(
        journal
            .prepare_scoped(callback_request("one"), callback_scope.clone())
            .await
            .unwrap(),
    );
    let second = ready(
        journal
            .prepare_scoped(callback_request("two"), callback_scope)
            .await
            .unwrap(),
    );
    let flight = journal.begin_scoped(first, context(10)).await.unwrap();
    journal
        .complete(
            flight,
            Completion::from_error(&tg::Error::handler("synthetic unknown")),
        )
        .await
        .unwrap();
    assert_eq!(
        journal
            .begin_scoped(second, context(10))
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::Conflict
    );
    let value: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT write_attempts FROM executor_budgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(value, 1);
}

#[tokio::test]
async fn new_attempt_clears_prior_not_sent_advice_and_spends_another_write_admission() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("attempts.sqlite");
    let journal = Journal::open(&path, JournalOptions::default())
        .await
        .unwrap();
    setup(&journal).await;
    let prepared = ready(
        journal
            .prepare_scoped(request("same"), scope())
            .await
            .unwrap(),
    );
    let first = journal.begin_scoped(prepared, context(3)).await.unwrap();
    journal
        .complete(first, Completion::executor_not_started())
        .await
        .unwrap();
    assert_eq!(
        journal
            .get("primary", "alice", "same")
            .await
            .unwrap()
            .unwrap()
            .failure()
            .unwrap()
            .outcome(),
        "not_sent"
    );
    let prepared = ready(
        journal
            .prepare_scoped(request("same"), scope())
            .await
            .unwrap(),
    );
    journal.begin_scoped(prepared, context(3)).await.unwrap();
    let current = journal
        .get("primary", "alice", "same")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.state(), OperationState::InFlight);
    assert!(current.failure().is_none());
    let attempts: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT write_attempts FROM executor_budgets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(attempts, 2);
}

async fn charge_at(
    journal: &Journal,
    context: BudgetContext,
    delta: Charge,
    time: i64,
) -> super::Result<()> {
    journal
        .run(move |connection, options, _| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| super::storage())?;
            charge(&transaction, options, &context, delta, time)?;
            transaction.commit().map_err(|_| super::storage())
        })
        .await
}

#[tokio::test]
async fn forward_windows_reset_usage_but_backward_windows_never_reopen_spent_buckets() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("windows.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let context = BudgetContext {
        budget: Budget::new("fixed-window")
            .unwrap()
            .window(Duration::from_secs(1))
            .limits(BudgetLimits {
                invocations: 1,
                ..BudgetLimits::default()
            }),
        ..context(1)
    };
    let delta = Charge {
        invocations: 1,
        ..Charge::default()
    };
    charge_at(&journal, context.clone(), delta, 1000)
        .await
        .unwrap();
    assert_eq!(
        charge_at(&journal, context.clone(), delta, 1999)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::QuotaExceeded
    );
    charge_at(&journal, context.clone(), delta, 2000)
        .await
        .unwrap();
    assert_eq!(
        charge_at(&journal, context.clone(), delta, 1000)
            .await
            .unwrap_err()
            .kind(),
        JournalErrorKind::QuotaExceeded
    );
    charge_at(&journal, context, delta, 3000).await.unwrap();
}

#[tokio::test]
async fn upload_cost_and_ledger_capacity_limits_fail_atomically_without_eviction_or_overflow() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("limits.sqlite");
    let journal = Journal::open(&path, JournalOptions::default().max_budget_ledgers(1))
        .await
        .unwrap();
    let context = BudgetContext {
        budget: Budget::new("limited").unwrap().limits(BudgetLimits {
            invocations: 2,
            write_attempts: 2,
            upload_bytes: 5,
            cost_units: 3,
        }),
        ..context(2)
    };
    charge_at(
        &journal,
        context.clone(),
        Charge {
            invocations: 1,
            upload_bytes: 3,
            cost_units: 2,
            ..Charge::default()
        },
        0,
    )
    .await
    .unwrap();
    for delta in [
        Charge {
            upload_bytes: 3,
            ..Charge::default()
        },
        Charge {
            cost_units: 2,
            ..Charge::default()
        },
        Charge {
            invocations: u64::MAX,
            ..Charge::default()
        },
    ] {
        assert_eq!(
            charge_at(&journal, context.clone(), delta, 0)
                .await
                .unwrap_err()
                .kind(),
            JournalErrorKind::QuotaExceeded
        );
    }
    let other = BudgetContext {
        budget: Budget::new("different-host-budget").unwrap(),
        ..context.clone()
    };
    assert_eq!(
        charge_at(
            &journal,
            other,
            Charge {
                invocations: 1,
                ..Charge::default()
            },
            0
        )
        .await
        .unwrap_err()
        .kind(),
        JournalErrorKind::Capacity
    );
    charge_at(
        &journal,
        context,
        Charge {
            invocations: 1,
            upload_bytes: 2,
            cost_units: 1,
            ..Charge::default()
        },
        0,
    )
    .await
    .unwrap();
    let usage: (i64, i64, i64, i64) = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT invocations,write_attempts,upload_bytes,cost_units FROM executor_budgets",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(usage, (2, 0, 5, 3));
}
