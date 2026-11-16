use super::*;
use crate::{
    Update,
    webhook::{WebhookError, WebhookSink},
};
use serde_json::json;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Notify;
fn event(id: i64, chat: Option<i64>) -> Update {
    serde_json::from_value(if let Some(chat)=chat{json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":chat,"type":"private"}}})}else{json!({"update_id":id,"future_event":{}})}).unwrap()
}
fn start<S: Send + Sync + 'static>(router: Router<S>, options: MemoryWebhookOptions) -> Driver {
    let (sink, receiver) = MemorySink::new(options.clone());
    let abort = CancellationToken::new();
    let task = tokio::spawn(worker::run(
        Bot::new("0:OFFLINE").unwrap(),
        Arc::new(router),
        sink.clone(),
        receiver,
        options,
        abort.clone(),
    ));
    Driver {
        sink,
        abort,
        task: Some(task),
    }
}
async fn pending(sink: &MemorySink, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while sink.pending_count() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn submit(
    driver: &Driver,
    event: Update,
    deadline: Instant,
) -> tokio::task::JoinHandle<queue::Outcome> {
    let sink = driver.sink.clone();
    tokio::spawn(async move { sink.ingest(event, deadline).await })
}

#[tokio::test]
async fn an_unknown_scope_waits_for_earlier_work_and_blocks_later_chat_admission() {
    let first_started = Arc::new(Notify::new());
    let first_release = Arc::new(Notify::new());
    let global_started = Arc::new(Notify::new());
    let global_release = Arc::new(Notify::new());
    let later_started = Arc::new(Notify::new());
    let router = Router::with_state((
        first_started.clone(),
        first_release.clone(),
        global_started.clone(),
        global_release.clone(),
        later_started.clone(),
    ))
    .on_update(|ctx| async move {
        match ctx.update.update_id {
            1 => {
                ctx.state().0.notify_one();
                ctx.state().1.notified().await;
            }
            2 => {
                ctx.state().2.notify_one();
                ctx.state().3.notified().await;
            }
            _ => ctx.state().4.notify_one(),
        }
        Ok(())
    });
    let mut driver = start(router, MemoryWebhookOptions::default());
    let deadline = Instant::now() + Duration::from_secs(3);
    let first = submit(&driver, event(1, Some(1)), deadline);
    tokio::time::timeout(Duration::from_secs(1), first_started.notified())
        .await
        .unwrap();
    let global = submit(&driver, event(2, None), deadline);
    pending(&driver.sink, 2).await;
    let later = submit(&driver, event(3, Some(2)), deadline);
    pending(&driver.sink, 3).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), global_started.notified())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), later_started.notified())
            .await
            .is_err()
    );
    first_release.notify_one();
    first.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(1), global_started.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), later_started.notified())
            .await
            .is_err()
    );
    global_release.notify_one();
    global.await.unwrap().unwrap();
    later.await.unwrap().unwrap();
    driver
        .finish(Instant::now() + Duration::from_secs(1), false)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_queued_deadline_expires_and_reclaims_its_slot_while_another_handler_is_busy() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state((started.clone(), release.clone(), calls.clone())).on_update(
        |ctx| async move {
            ctx.state().2.fetch_add(1, Ordering::SeqCst);
            if ctx.update.update_id == 1 {
                ctx.state().0.notify_one();
                ctx.state().1.notified().await;
            }
            Ok(())
        },
    );
    let options = MemoryWebhookOptions::default()
        .max_pending(2)
        .dispatch(crate::DispatchOptions::default().max_concurrency(1));
    let mut driver = start(router, options);
    let deadline = Instant::now() + Duration::from_secs(2);
    let first = submit(&driver, event(1, Some(1)), deadline);
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    let expired = submit(
        &driver,
        event(2, Some(2)),
        Instant::now() + Duration::from_millis(20),
    );
    pending(&driver.sink, 2).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(250), expired)
            .await
            .expect("waiting for the active handler must not delay a queued request's deadline")
            .unwrap(),
        Err(WebhookError::Timeout)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let later = submit(&driver, event(3, Some(2)), deadline);
    pending(&driver.sink, 2).await;
    release.notify_one();
    first.await.unwrap().unwrap();
    later.await.unwrap().unwrap();
    driver
        .finish(Instant::now() + Duration::from_secs(1), false)
        .await
        .unwrap();
}

#[tokio::test]
async fn canonical_fingerprints_ignore_object_order_and_payload_bytes_are_reclaimed() {
    let first: Update =
        serde_json::from_str(r#"{"update_id":1,"future":{"b":2,"a":{"x":3,"y":4}}}"#).unwrap();
    let reordered: Update =
        serde_json::from_str(r#"{"future":{"a":{"y":4,"x":3},"b":2},"update_id":1}"#).unwrap();
    let mut canonical = serde_json::to_value(&first).unwrap();
    canonical.sort_all_objects();
    let bytes = serde_json::to_vec(&canonical).unwrap().len();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::with_state(calls.clone()).on_update(|ctx| async move {
        ctx.state().fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut driver = start(
        router,
        MemoryWebhookOptions::default().max_pending_bytes(bytes),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    driver.sink.ingest(first, deadline).await.unwrap();
    driver.sink.ingest(reordered, deadline).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let too_large: Update =
        serde_json::from_value(json!({"update_id":2,"future":"x".repeat(bytes)})).unwrap();
    assert_eq!(
        driver.sink.ingest(too_large, deadline).await,
        Err(WebhookError::Busy)
    );
    driver
        .sink
        .ingest(
            serde_json::from_value(json!({"update_id":3})).unwrap(),
            deadline,
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    driver
        .finish(Instant::now() + Duration::from_secs(1), false)
        .await
        .unwrap();
}
