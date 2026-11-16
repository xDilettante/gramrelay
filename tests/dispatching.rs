#![cfg(feature = "bot")]
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tg::{Bot, DispatchOptions, ErrorKind, Router, Update};
use tokio::sync::Notify;

fn event(id: i64, chat: i64) -> Update {
    serde_json::from_value(json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"}}})).unwrap()
}

#[test]
fn managed_sources_share_ownership_across_rotated_tokens_and_normalized_ids() {
    let first = Bot::new("009998801:SYNTHETIC_A").unwrap();
    let rotated = Bot::new("9998801:SYNTHETIC_B").unwrap();
    let guard = first.claim_update_source().unwrap();
    assert_eq!(
        rotated.claim_update_source().unwrap_err().kind(),
        ErrorKind::Capacity
    );
    assert!(!format!("{guard:?}").contains("9998801"));
    drop(guard);
    rotated.claim_update_source().unwrap();
}

#[tokio::test]
async fn manual_batches_preserve_chat_order_and_bound_concurrency_without_network() {
    let bot = Bot::new("9998802:SYNTHETIC").unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let first_started = Arc::new(Notify::new());
    let other_finished = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let state = (
        seen.clone(),
        first_started.clone(),
        other_finished.clone(),
        release.clone(),
    );
    let router = Arc::new(Router::with_state(state).on_update(|ctx| async move {
        let (seen, started, other, release) = ctx.state();
        let id = ctx.update.update_id;
        if id == 1 {
            started.notify_one();
            release.notified().await;
        }
        seen.lock().unwrap().push(id);
        if id == 3 {
            other.notify_one();
        }
        Ok(())
    }));
    let task = tokio::spawn(async move {
        bot.dispatch_batch(
            router,
            vec![event(1, 1), event(2, 1), event(3, 2), event(4, 3)],
            &DispatchOptions::default().max_concurrency(2),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), other_finished.notified())
        .await
        .unwrap();
    assert!(
        !seen.lock().unwrap().contains(&2),
        "the next event of the blocked chat must not be admitted"
    );
    release.notify_one();
    task.await.unwrap().unwrap();
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().position(|id| *id == 1).unwrap() < seen.iter().position(|id| *id == 2).unwrap()
    );
}

#[tokio::test]
async fn invalid_batches_are_rejected_before_any_handler_and_timeouts_are_bounded() {
    let bot = Bot::new("9998803:SYNTHETIC").unwrap();
    let router = Arc::new(
        Router::new().on_update(|_| async { panic!("invalid batch must not reach a handler") }),
    );
    let mut malformed = event(2, 1);
    malformed.edited_message = malformed.message.clone();
    assert_eq!(
        bot.dispatch_batch(
            router.clone(),
            vec![event(1, 1), malformed],
            &DispatchOptions::default()
        )
        .await
        .unwrap_err()
        .kind(),
        ErrorKind::Validation
    );
    assert_eq!(
        bot.dispatch_batch(
            router,
            vec![],
            &DispatchOptions::default().max_concurrency(0)
        )
        .await
        .unwrap_err()
        .kind(),
        ErrorKind::Validation
    );
    let router = Arc::new(Router::new().on_update(|_| std::future::pending()));
    assert_eq!(
        bot.dispatch_batch(
            router,
            vec![event(1, 1)],
            &DispatchOptions::default().handler_timeout(Duration::from_millis(10))
        )
        .await
        .unwrap_err()
        .kind(),
        ErrorKind::Timeout
    );
}

#[tokio::test]
async fn cancellation_joins_running_handlers_without_admitting_pending_groups() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tg::CancellationToken;
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let bot = Bot::new("9998810:SYNTHETIC").unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let router = Arc::new(
        Router::with_state((active.clone(), started.clone())).on_update(|ctx| async move {
            assert_ne!(
                ctx.update.update_id, 3,
                "pending groups must not be admitted after cancellation"
            );
            ctx.state().0.fetch_add(1, Ordering::SeqCst);
            let _active = Active(ctx.state().0.clone());
            ctx.state().1.add_permits(1);
            std::future::pending().await
        }),
    );
    let stop = CancellationToken::new();
    let signal = stop.clone();
    let task = tokio::spawn(async move {
        bot.dispatch_batch_until(
            router,
            vec![event(1, 1), event(2, 2), event(3, 3)],
            &DispatchOptions::default().max_concurrency(2),
            signal,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), started.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    stop.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let bot = Bot::new("9998811:SYNTHETIC").unwrap();
    let stop = CancellationToken::new();
    stop.cancel();
    let error = bot
        .dispatch_batch_until(
            Arc::new(Router::new()),
            vec![event(1, 1)],
            &DispatchOptions::default(),
            stop,
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), tg::EffectOutcome::NotSent);
}
