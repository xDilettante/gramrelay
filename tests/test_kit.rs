#![cfg(feature = "test-kit")]

#[cfg(feature = "bot")]
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tg::{
    ChatId, EffectOutcome, ErrorKind, InputFile, RetryPolicy,
    api::methods,
    test_kit::{MockResponse, MockTransport, Step},
};

fn sent() -> MockResponse {
    MockResponse::json(
        json!({"ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}}),
    )
}
fn identity() -> MockResponse {
    MockResponse::json(json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}))
}
fn policy() -> RetryPolicy {
    RetryPolicy::default()
        .initial_delay(Duration::ZERO)
        .max_delay(Duration::from_millis(1))
        .jitter(false)
}

#[tokio::test]
async fn lazy_requests_use_real_json_and_typed_records_without_network() {
    let mock = MockTransport::new([Step::reply("sendMessage", sent())]).unwrap();
    let bot = mock
        .bot_builder()
        .api_url("https://must-never-resolve.invalid/")
        .build()
        .unwrap();
    let request = bot.send_message(ChatId::from(1), "PRIVATE_TEXT");
    assert!(mock.requests().unwrap().is_empty());
    assert_eq!(request.await.unwrap().message_id, 42.into());
    let records = mock.requests().unwrap();
    let recorded = &records[0];
    assert_eq!(recorded.method(), "sendMessage");
    assert_eq!(recorded.http_method(), "POST");
    assert_eq!(recorded.content_type(), Some("application/json"));
    assert!(recorded.upload_complete());
    let params = recorded.decode::<methods::SendMessage>().unwrap();
    assert_eq!(params.text, "PRIVATE_TEXT");
    assert_eq!(
        recorded.decode::<methods::GetMe>().unwrap_err().outcome(),
        EffectOutcome::NotSent
    );
    assert!(
        !format!(
            "{recorded:?} {mock:?} {request_debug:?}",
            request_debug = bot
        )
        .contains("PRIVATE_TEXT")
    );
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn multipart_uses_the_real_stream_and_can_fail_after_a_prefix() {
    let mock = MockTransport::new([
        Step::reply("sendDocument", sent()),
        Step::partial_upload("sendDocument", 37),
    ])
    .unwrap();
    let bot = mock.bot().unwrap();
    bot.send_document(
        ChatId::from(1),
        InputFile::bytes("PRIVATE_NAME.bin", vec![0, 255, 1]),
    )
    .await
    .unwrap();
    let error = bot
        .send_document(
            ChatId::from(1),
            InputFile::bytes("other.bin", vec![5; 1000]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let requests = mock.requests().unwrap();
    assert!(
        requests[0]
            .content_type()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert!(String::from_utf8_lossy(requests[0].body()).contains("filename=\"PRIVATE_NAME.bin\""));
    assert!(
        requests[0]
            .body()
            .windows(3)
            .any(|bytes| bytes == [0, 255, 1])
    );
    assert!(requests[0].upload_complete());
    assert_eq!(requests[1].body().len(), 37);
    assert!(!requests[1].upload_complete());
    assert!(!format!("{requests:?}").contains("PRIVATE_NAME"));
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn refusals_and_read_retries_use_shared_classification() {
    let mock = MockTransport::new([
        Step::reply("getMe", MockResponse::json(json!({"ok":false,"error_code":429,"description":"PRIVATE_REMOTE","parameters":{"retry_after":0}})).status(429)),
        Step::reply("getMe", identity()),
        Step::reply("sendMessage", MockResponse::json(json!({"ok":false,"error_code":429,"parameters":{"retry_after":7}})).status(429)),
    ]).unwrap();
    let bot = mock.bot().unwrap();
    bot.api().get_me().retry_policy(policy()).await.unwrap();
    let error = bot
        .send_message(ChatId::from(1), "one attempt")
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Rejected);
    assert_eq!(error.retry_after(), Some(Duration::from_secs(7)));
    let requests = mock.requests().unwrap();
    assert_eq!(requests[0].body(), requests[1].body());
    assert!(!format!("{error:?}").contains("PRIVATE_REMOTE"));
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn timeout_unknown_write_never_replays_and_releases_capacity() {
    let mock = MockTransport::new([
        Step::timeout("sendMessage"),
        Step::reply("getMe", identity()),
    ])
    .unwrap();
    let bot = mock
        .bot_builder()
        .timeout(Duration::from_millis(15))
        .max_in_flight(1)
        .build()
        .unwrap();
    let error = bot
        .api()
        .send_message("once".into())
        .chat_id(ChatId::from(1).into())
        .retry_policy(policy().retry_rejected_writes(true))
        .await
        .unwrap_err();
    assert_eq!(
        (error.kind(), error.outcome()),
        (ErrorKind::Timeout, EffectOutcome::Unknown)
    );
    bot.get_me().await.unwrap();
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn dropping_a_future_drops_only_its_claimed_step_and_releases_capacity() {
    let mock =
        MockTransport::new([Step::timeout("getMe"), Step::reply("getMe", identity())]).unwrap();
    let bot = mock.bot_builder().max_in_flight(1).build().unwrap();
    let mut future = Box::pin(bot.get_me());
    tokio::select! {
        result = &mut future => panic!("unexpected result: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    assert_eq!(mock.requests().unwrap().len(), 1);
    assert_eq!(bot.get_me().await.unwrap_err().kind(), ErrorKind::Capacity);
    drop(future);
    bot.get_me().await.unwrap();
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn chunked_downloads_and_corruption_use_the_production_decoder() {
    let mock = MockTransport::new([
        Step::reply(
            "download",
            MockResponse::chunks([b"abc".to_vec(), b"def".to_vec()]),
        ),
        Step::reply(
            "download",
            MockResponse::chunks([b"abc".to_vec(), b"def".to_vec()]).fail_after_chunks(1),
        ),
        Step::reply(
            "getMe",
            MockResponse::bytes(b"malformed PRIVATE_RESPONSE".to_vec()),
        ),
        Step::reply("getMe", identity().chunk_delay(Duration::from_millis(100))),
    ])
    .unwrap();
    let bot = mock
        .bot_builder()
        .timeout(Duration::from_millis(15))
        .max_download_bytes(4)
        .build()
        .unwrap();
    let file = serde_json::from_value(
        json!({"file_id":"file","file_unique_id":"unique","file_path":"files/offline"}),
    )
    .unwrap();
    let mut output = Vec::new();
    assert_eq!(
        bot.download_file(&file)
            .to_writer(&mut output)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::ResponseTooLarge
    );
    assert_eq!(output, b"abc");
    assert_eq!(
        bot.download_file(&file).bytes().await.unwrap_err().kind(),
        ErrorKind::Transport
    );
    let error = bot.get_me().await.unwrap_err();
    assert_eq!(
        (error.kind(), error.outcome()),
        (ErrorKind::Decode, EffectOutcome::Unknown)
    );
    assert!(!format!("{error:?}").contains("PRIVATE_RESPONSE"));
    assert_eq!(bot.get_me().await.unwrap_err().kind(), ErrorKind::Timeout);
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn scripts_fail_closed_and_all_memory_budgets_are_enforced() {
    let mock = MockTransport::new([Step::reply("getMe", identity())]).unwrap();
    let bot = mock.bot().unwrap();
    assert!(mock.assert_finished().is_err());
    assert_eq!(
        bot.send_message(ChatId::from(1), "unexpected")
            .await
            .unwrap_err()
            .outcome(),
        EffectOutcome::NotSent
    );
    bot.get_me().await.unwrap();
    assert_eq!(
        bot.get_me().await.unwrap_err().outcome(),
        EffectOutcome::NotSent
    );
    mock.assert_finished().unwrap();
    assert!(MockTransport::with_limits([Step::reply("getMe", identity())], 0, 10).is_err());
    assert!(MockTransport::with_limits([Step::reply("getMe", identity())], 1, 5).is_err());
    assert!(
        MockTransport::with_limits([Step::timeout("getMe"), Step::timeout("getMe")], 1, 100)
            .is_err()
    );
    assert!(MockTransport::new([Step::reply("typo", identity())]).is_err());
    let bounded = MockTransport::with_limits([Step::reply("sendMessage", sent())], 1, 128).unwrap();
    let error = bounded
        .bot()
        .unwrap()
        .send_message(ChatId::from(1), "x".repeat(300))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Capacity);
    assert!(
        bounded
            .requests()
            .unwrap()
            .iter()
            .map(|request| request.body().len())
            .sum::<usize>()
            <= 128
    );
}

#[tokio::test]
async fn transport_failure_is_ambiguous_and_a_read_can_recover() {
    let mock = MockTransport::new([
        Step::transport_error("getMe"),
        Step::reply("getMe", identity()),
    ])
    .unwrap();
    mock.bot()
        .unwrap()
        .api()
        .get_me()
        .retry_policy(policy())
        .await
        .unwrap();
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn a_real_file_stream_can_be_interrupted_inside_its_contents() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("PRIVATE_INPUT.bin");
    std::fs::write(&path, vec![0xff; 4096]).unwrap();
    let mock = MockTransport::new([Step::partial_upload("sendDocument", 1024)]).unwrap();
    let error = mock
        .bot()
        .unwrap()
        .send_document(ChatId::from(1), InputFile::path(&path))
        .await
        .unwrap_err();
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let requests = mock.requests().unwrap();
    assert_eq!(requests[0].body().len(), 1024);
    assert!(
        requests[0]
            .body()
            .windows(64)
            .any(|bytes| bytes.iter().all(|byte| *byte == 0xff))
    );
    assert!(!requests[0].upload_complete());
    std::fs::remove_file(path).unwrap();
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn parallel_requests_claim_steps_atomically_without_holding_a_lock_across_await() {
    let mock = MockTransport::new([
        Step::reply("getMe", identity().chunk_delay(Duration::from_secs(5))),
        Step::reply("getMe", identity()),
    ])
    .unwrap();
    let bot = mock.bot().unwrap();
    let mut first = Box::pin(bot.get_me());
    tokio::select! {
        biased;
        result = &mut first => panic!("unexpected response: {result:?}"),
        _ = std::future::ready(()) => {}
    }
    assert!(mock.assert_finished().is_err());
    bot.get_me().await.unwrap();
    assert!(mock.assert_finished().is_err());
    drop(first);
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
}

#[tokio::test]
async fn preparation_errors_do_not_claim_steps_and_history_has_a_total_byte_cap() {
    let mock = MockTransport::new([Step::reply("sendDocument", sent())]).unwrap();
    let bot = mock.bot().unwrap();
    assert_eq!(
        bot.send_document(ChatId::from(1), InputFile::path("PRIVATE_MISSING_INPUT"))
            .await
            .unwrap_err()
            .outcome(),
        EffectOutcome::NotSent
    );
    assert!(mock.requests().unwrap().is_empty());
    bot.send_document(ChatId::from(1), InputFile::bytes("ok.bin", vec![1]))
        .await
        .unwrap();
    mock.assert_finished().unwrap();

    let mock = MockTransport::with_limits(
        [
            Step::reply("sendMessage", sent()),
            Step::reply("sendMessage", sent()),
        ],
        2,
        256,
    )
    .unwrap();
    let bot = mock.bot().unwrap();
    bot.send_message(ChatId::from(1), "a".repeat(200))
        .await
        .unwrap();
    assert_eq!(
        bot.send_message(ChatId::from(1), "b".repeat(200))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Capacity
    );
    let requests = mock.requests().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.body().len())
            .sum::<usize>(),
        256
    );
    assert!(!requests[1].upload_complete());
    mock.assert_finished().unwrap();
    assert!(MockTransport::new([Step::reply("getMe", identity().status(999))]).is_err());
    assert!(MockTransport::new([Step::reply("getMe", identity().delay(Duration::MAX))]).is_err());
    assert!(
        MockTransport::with_limits(
            [Step::reply(
                "getMe",
                MockResponse::chunks(vec![vec![]; 129])
            )],
            1,
            128
        )
        .is_err()
    );
}

#[cfg(feature = "bot")]
#[tokio::test]
async fn handlers_can_replay_reordered_update_fixtures() {
    let updates: Vec<Value> = [2, 1].into_iter().map(|id| json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":1,"type":"private"},"text":format!("event {id}")}})).collect();
    let mock = MockTransport::new([
        Step::reply(
            "getUpdates",
            MockResponse::json(json!({"ok":true,"result":updates})),
        ),
        Step::reply("sendMessage", sent()),
        Step::reply("sendMessage", sent()),
    ])
    .unwrap();
    let bot = mock.bot().unwrap();
    let router = tg::Router::new().on_message(|ctx: tg::MessageContext| async move {
        ctx.reply(ctx.message.text.clone().unwrap()).await?;
        Ok(())
    });
    for update in bot.get_updates().await.unwrap() {
        router.dispatch(&bot, update).await.unwrap();
    }
    let requests = mock.requests().unwrap();
    assert_eq!(
        requests[1].decode::<methods::SendMessage>().unwrap().text,
        "event 2"
    );
    assert_eq!(
        requests[2].decode::<methods::SendMessage>().unwrap().text,
        "event 1"
    );
    mock.assert_finished().unwrap();
}
