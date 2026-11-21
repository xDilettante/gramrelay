//! Public consumer acceptance, copied into an independent Cargo workspace.
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg::{
    ButtonStyle, CallbackContext, CallbackQueryId, CancellationToken, ChatId, EffectOutcome,
    ErrorKind, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, PollingOptions,
    PollingReconnect, RichDoc, Router,
    rich_message::HeadingLevel,
    test_kit::{MockResponse, MockTransport, Step},
};
use tg_agent::{
    executor::{Executor, ExecutorOptions, PolicyStore, Principal},
    mcp::McpServer,
    tools::{ExecutionMode, OutputLimit, ToolRequest, ToolSession, extended_catalog},
};
use tg_mtproto::{
    ClientOptions, Session, SessionBinding, SessionKey, SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(15), exercise()).await??;
    println!(
        "external fixture: Bot message/callback/RichDoc/upload/reconnect/Unknown, MCP discovery, MT real-TL and joined close passed"
    );
    Ok(())
}

async fn exercise() -> Result<(), Box<dyn std::error::Error>> {
    let message =
        json!({"ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}});
    let mock = MockTransport::new([
        Step::reply("sendMessage", MockResponse::json(message.clone())),
        Step::reply(
            "answerCallbackQuery",
            MockResponse::json(json!({"ok":true,"result":true})),
        ),
        Step::reply("sendDocument", MockResponse::json(message.clone())),
        Step::reply("sendRichMessage", MockResponse::json(message)),
        Step::timeout("sendMessage"),
    ])?;
    let bot = mock
        .bot_builder()
        .timeout(Duration::from_millis(50))
        .build()?;
    assert_eq!(
        i64::from(
            bot.send_message(ChatId::from(1), "external")
                .await?
                .message_id
        ),
        42
    );
    bot.answer_callback_query(CallbackQueryId::from("synthetic-callback"))
        .text("done")
        .await?;
    bot.send_document(
        ChatId::from(1),
        InputFile::bytes("fixture.bin", vec![1, 2, 3]),
    )
    .await?;
    let report = RichDoc::new()
        .heading(HeadingLevel::H2, "External report")
        .photo_attach("chart", None);
    let keyboard = InlineKeyboardMarkup::new().row([
        InlineKeyboardButton::url("Details", "https://example.com").style(ButtonStyle::Primary),
        InlineKeyboardButton::copy_text("Copy", "external"),
    ]);
    bot.send_rich_message(ChatId::from(1), report)
        .reply_markup(keyboard)
        .attach(
            "chart",
            InputFile::bytes("chart.png", b"LOCAL_CHART".to_vec()),
        )
        .await?;
    let error = bot
        .send_message(ChatId::from(1), "uncertain")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_eq!(error.outcome(), EffectOutcome::Unknown);
    let requests = mock.requests()?;
    assert_eq!(requests.len(), 5);
    let rich = &requests[3];
    assert_eq!(rich.method(), "sendRichMessage");
    assert!(rich.upload_complete());
    let body = std::str::from_utf8(rich.body())?;
    assert!(body.contains("attach://chart") && body.contains("LOCAL_CHART"));
    assert!(body.contains(r#""style":"primary""#) && body.contains("copy_text"));
    mock.assert_finished()?;

    exercise_polling().await?;

    let catalog = extended_catalog();
    assert_eq!(catalog.tools.len(), 16);
    for tool in &catalog.tools {
        assert_eq!(tool.output_schema["type"], "object");
        ToolRequest::from_json(&serde_json::to_vec(&tool.examples[0]["input"])?)?;
    }
    let executor = Executor::new(PolicyStore::default(), ExecutorOptions::default())?;
    let server = McpServer::new(
        ToolSession::new(executor, Principal::new("external")?),
        ExecutionMode::DryRun,
        OutputLimit::default(),
    );
    let (client_io, server_io) = tokio::io::duplex(262144);
    let task = tokio::spawn(server.serve(server_io));
    let (read, mut write) = tokio::io::split(client_io);
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"external","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let mut frame = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":meta}}),
    )?;
    frame.push(b'\n');
    write.write_all(&frame).await?;
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let response: Value = serde_json::from_str(&line)?;
    assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 8);
    drop(reader);
    drop(write);
    task.await??;

    let directory = tempfile::tempdir()?;
    let session = Session::open(
        directory.path().join("session.sqlite"),
        SessionBinding::new("external", 1)?,
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let client = test_kit::client(
        session.clone(),
        ClientOptions::default(),
        move |_, bytes| {
            let count = count.clone();
            async move {
                assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
                assert_eq!(
                    bytes[..4],
                    tl::functions::Ping::CONSTRUCTOR_ID.to_le_bytes()
                );
                let ping = tl::functions::Ping::from_bytes(&bytes[4..]).unwrap();
                Ok(tl::enums::Pong::Pong(tl::types::Pong {
                    msg_id: 1,
                    ping_id: ping.ping_id,
                })
                .to_bytes())
            }
        },
    )?;
    let tl::enums::Pong::Pong(pong) = client.invoke(&tl::functions::Ping { ping_id: 17 }).await?;
    assert_eq!(pong.ping_id, 17);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    client.close().await?;
    assert_eq!(
        client
            .invoke(&tl::functions::Ping { ping_id: 18 })
            .await
            .unwrap_err()
            .outcome(),
        tg_mtproto::Outcome::NotSent
    );
    assert_eq!(
        session.remembered_account().unwrap_err().kind(),
        tg_mtproto::SessionErrorKind::Closed
    );
    Ok(())
}

async fn exercise_polling() -> Result<(), Box<dyn std::error::Error>> {
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(json!({"ok":true,"result":{
                "id":0,"is_bot":true,"first_name":"External","username":"external_bot"
            }})),
        ),
        Step::reply(
            "getWebhookInfo",
            MockResponse::json(json!({"ok":true,"result":{
                "url":"","has_custom_certificate":false,"pending_update_count":0
            }})),
        ),
        Step::transport_error("getUpdates"),
        Step::reply(
            "getUpdates",
            MockResponse::json(json!({"ok":true,"result":[{
                "update_id":1,"callback_query":{"id":"external-callback","chat_instance":"external",
                    "from":{"id":1,"is_bot":false,"first_name":"User"},"data":"chart:BTC"}
            }]})),
        ),
    ])?;
    let stop = CancellationToken::new();
    let router = Router::with_state(stop.clone()).callback_prefix(
        "chart:",
        |ctx: CallbackContext<CancellationToken>| async move {
            assert_eq!(ctx.query.data.as_deref(), Some("chart:BTC"));
            ctx.state().cancel();
            Ok(())
        },
    );
    let reconnect = PollingReconnect::default().initial_delay(Duration::from_millis(1));
    let options = PollingOptions::default()
        .poll_timeout(0)
        .reconnect(reconnect);
    mock.bot()?.run_polling_until(router, options, stop).await?;
    mock.assert_finished()?;
    Ok(())
}
