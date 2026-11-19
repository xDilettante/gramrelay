use super::*;
use crate::CallbackAnswer;
fn bot() -> AccountIdentity {
    AccountIdentity {
        kind: AccountKind::Bot,
        ..expected()
    }
}
fn bot_reply(bytes: &[u8]) -> Option<Vec<u8>> {
    (bytes[..4] == tl::functions::users::GetUsers::CONSTRUCTOR_ID.to_le_bytes())
        .then(|| vec![user(42, (1 << 10) | (1 << 14), 0)].to_bytes())
}
#[test]
fn callback_intents_bound_plain_text_and_redact_query_metadata() {
    assert!(CallbackAnswer::new(0).text("😀".repeat(100)).is_ok());
    assert_eq!(
        CallbackAnswer::new(0)
            .text("😀".repeat(101))
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    assert_eq!(
        CallbackAnswer::new(0)
            .text("private\0text")
            .unwrap_err()
            .outcome(),
        Outcome::NotSent
    );
    let answer = CallbackAnswer::new(1239876543210)
        .text("PRIVATE_ANSWER")
        .unwrap();
    let debug = format!("{answer:?}");
    assert!(!debug.contains("PRIVATE_"));
    assert!(!debug.contains("1239876543210"));
}
#[tokio::test]
async fn callback_wire_accepts_full_opaque_query_ids_and_confirms_only_true_without_urls_or_cache()
{
    let (_dir, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            if let Some(reply) = bot_reply(&bytes) {
                return Ok(reply);
            }
            assert_eq!(
                bytes[..4],
                tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID.to_le_bytes()
            );
            let request =
                tl::functions::messages::SetBotCallbackAnswer::from_bytes(&bytes[4..]).unwrap();
            let index = seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.query_id, [0, i64::MIN, i64::MAX][index]);
            assert!(request.alert);
            assert_eq!(request.message.as_deref(), Some("PRIVATE_ANSWER"));
            assert!(request.url.is_none());
            assert_eq!(request.cache_time, 0);
            Ok(true.to_bytes())
        }
    });
    let account = client.account(bot());
    for id in [0, i64::MIN, i64::MAX] {
        account
            .answer_callback(
                CallbackAnswer::new(id)
                    .text("PRIVATE_ANSWER")
                    .unwrap()
                    .show_alert(true),
            )
            .await
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let error = account
        .answer_callback_until(
            CallbackAnswer::new(0),
            std::time::Instant::now() - Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), Outcome::NotSent);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    client.close().await.unwrap();
}
#[tokio::test]
async fn user_callback_intent_is_lazy_and_refused_without_any_rpc() {
    let (_dir, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        async { Ok(true.to_bytes()) }
    });
    let account = client.account(expected());
    drop(account.answer_callback(CallbackAnswer::new(0)));
    let error = account
        .answer_callback(CallbackAnswer::new(0))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ClientErrorKind::UnsupportedOperation);
    assert_eq!(error.outcome(), Outcome::NotSent);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    client.close().await.unwrap();
}

#[tokio::test]
async fn callback_false_rpc_and_timeout_are_classified_without_replay_or_remote_diagnostics() {
    for mode in 0..4 {
        let (_dir, session) = session().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let client = fixture(
            session,
            ClientOptions::default().request_timeout(Duration::from_millis(150)),
            move |_, bytes| {
                let seen = seen.clone();
                async move {
                    if let Some(reply) = bot_reply(&bytes) {
                        return Ok(reply);
                    }
                    seen.fetch_add(1, Ordering::SeqCst);
                    match mode {
                        0 => Ok(false.to_bytes()),
                        1 => Err(grammers_mtsender::InvocationError::Rpc(
                            tl::types::RpcError {
                                error_code: 400,
                                error_message: "PRIVATE_CALLBACK_ERROR".into(),
                            }
                            .into(),
                        )),
                        2 => std::future::pending().await,
                        _ => Ok(vec![0; 4]),
                    }
                }
            },
        );
        let error = client
            .account(bot())
            .answer_callback(CallbackAnswer::new(0))
            .await
            .unwrap_err();
        assert_eq!(
            error.outcome(),
            if mode == 1 {
                Outcome::Rejected
            } else {
                Outcome::Unknown
            }
        );
        assert!(!format!("{error:?} {error}").contains("PRIVATE_"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        client.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn callback_absolute_deadline_covers_live_identity_without_target_dispatch() {
    let (_dir, session) = session().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let client = fixture(session, ClientOptions::default(), move |_, bytes| {
        let seen = seen.clone();
        async move {
            if bot_reply(&bytes).is_some() {
                std::future::pending::<()>().await;
            }
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(true.to_bytes())
        }
    });
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.account(bot()).answer_callback_until(
            CallbackAnswer::new(0),
            std::time::Instant::now() + Duration::from_millis(80),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(result.kind(), ClientErrorKind::Timeout);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    client.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_callback_caller_keeps_owned_capacity_and_close_joins_one_dispatch() {
    let (_dir, session) = session().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let ready = entered.clone();
    let blocked = release.clone();
    let seen = calls.clone();
    let client = fixture(
        session,
        ClientOptions::default().max_pending(1),
        move |_, bytes| {
            let (ready, blocked, seen) = (ready.clone(), blocked.clone(), seen.clone());
            async move {
                if let Some(reply) = bot_reply(&bytes) {
                    return Ok(reply);
                }
                seen.fetch_add(1, Ordering::SeqCst);
                ready.notify_one();
                blocked.notified().await;
                Ok(true.to_bytes())
            }
        },
    );
    let caller = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .account(bot())
                .answer_callback(CallbackAnswer::new(0))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let refused = client
        .account(bot())
        .answer_callback(CallbackAnswer::new(1))
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), ClientErrorKind::Capacity);
    let mut closing = tokio::spawn({
        let client = client.clone();
        async move { client.close().await }
    });
    // Close can conservatively abort the single owned RPC rather than wait for the remote reply.
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), &mut closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
