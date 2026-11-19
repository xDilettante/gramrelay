#![cfg(feature = "mcp")]
use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use tg_agent::{
    executor::{Budget, Executor, ExecutorOptions, Grant, PolicyStore, Principal},
    mcp::McpServer,
    tools::{ExecutionMode, OutputLimit, ToolSession, catalog},
};

fn no_profile_server(limit: OutputLimit) -> McpServer {
    McpServer::new(
        ToolSession::new(
            Executor::new(PolicyStore::default(), ExecutorOptions::default()).unwrap(),
            Principal::new("host").unwrap(),
        ),
        ExecutionMode::Execute,
        limit,
    )
}

#[tokio::test]
async fn current_protocol_works_without_legacy_initialize() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client_io, server_io) = tokio::io::duplex(262144);
    let server_task = tokio::spawn(no_profile_server(OutputLimit::default()).serve(server_io));
    let (mut read, mut write) = tokio::io::split(client_io);
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"offline","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let mut frame = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":meta}}),
    )
    .unwrap();
    frame.push(b'\n');
    write.write_all(&frame).await.unwrap();
    let mut reader = BufReader::new(&mut read);
    let mut line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        reader.read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    let first: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(first["id"], 1);
    assert_eq!(first["result"]["resultType"], "complete");
    assert!(first["result"]["ttlMs"].is_u64());
    assert!(first["result"]["cacheScope"].is_string());
    assert_eq!(first["result"]["tools"].as_array().unwrap().len(), 8);
    for (actual, expected) in first["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .zip(catalog().tools)
    {
        assert_eq!(actual["outputSchema"], expected.output_schema);
        assert_eq!(actual["outputSchema"]["type"], "object");
    }
    frame = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":meta}}),
    )
    .unwrap();
    frame.push(b'\n');
    write.write_all(&frame).await.unwrap();
    line.clear();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        reader.read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 2);
    drop(write);
    drop(reader);
    drop(read);
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn oversized_and_duplicate_raw_objects_close_before_sdk_map_normalization() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut frames = vec![
        br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_me","arguments":{"schema_version":0,"schema_version":1,"profile":"demo","call":{"tool":"get_me","arguments":{}}}}}
"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_chat","arguments":{"schema_version":1,"profile":"demo","call":{"tool":"get_chat","arguments":{"chat_id":"2","chat\u005fid":"1"}}}}}
"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}
"#.to_vec(),
    ];
    // SDK compatibility strips a UTF-8 BOM; the raw guard must inspect the
    // same JSON rather than treating that prefix as an ignorable parse error.
    let mut with_bom = vec![0xef, 0xbb, 0xbf];
    with_bom.extend_from_slice(&frames[0]);
    frames.push(with_bom);
    frames.push(vec![b' '; tg_agent::mcp::MAX_FRAME_BYTES + 1]);
    for frame in frames {
        let (client_io, server_io) = tokio::io::duplex(262144);
        let server_task = tokio::spawn(no_profile_server(OutputLimit::default()).serve(server_io));
        let (mut read, mut write) = tokio::io::split(client_io);
        let _ = write.write_all(&frame).await;
        let mut output = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            read.read_to_end(&mut output),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.is_empty(),
            "unvalidated arguments must not reach the executor"
        );
        assert_eq!(
            server_task.await.unwrap().unwrap_err().code(),
            "agent_mcp_transport"
        );
    }
}

#[tokio::test]
async fn bounded_output_returns_a_structured_delivery_error_without_remote_content() {
    use tg::test_kit::MockTransport;
    use tg_agent::{
        executor::{Action, ChatId, ChatScope},
        journal::{Journal, JournalOptions},
        updates::{UpdateFeed, UpdateOptions},
    };
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        directory.path().join("output.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let principal = Principal::new("host").unwrap();
    let policy = PolicyStore::default();
    policy
        .replace(
            &principal,
            "demo",
            Grant::new(0, Budget::new("budget").unwrap())
                .allow(Action::ReadUpdates)
                .chats(ChatScope::only([ChatId::new(1).unwrap()])),
        )
        .unwrap();
    let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
    let mock = MockTransport::new([]).unwrap();
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed
        .claim_source(tg_agent::updates::SourceMode::Webhook)
        .await
        .unwrap();
    feed.append_webhook(&source, serde_json::from_value(json!({"update_id":1,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE".repeat(1000)}})).unwrap()).await.unwrap();
    feed.release_source(source).await.unwrap();
    executor
        .register_profile(feed, mock.bot().unwrap())
        .unwrap();
    let server = McpServer::new(
        ToolSession::new(executor, principal),
        ExecutionMode::Execute,
        OutputLimit::new(1024).unwrap(),
    );
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server_task = tokio::spawn(server.serve(server_io));
    let client = ().serve(client_io).await.unwrap();
    let result = client.call_tool(CallToolRequestParams::new("read_updates").with_arguments(json!({"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{"limit":1}}}).as_object().unwrap().clone())).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    let value = result.structured_content.as_ref().unwrap();
    assert_eq!(value["error"]["code"], "agent_output_too_large");
    assert!(serde_json::to_vec(value).unwrap().len() <= 1024);
    assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE"));
    assert!(mock.requests().unwrap().is_empty());
    mock.assert_finished().unwrap();
    client.cancel().await.unwrap();
    server_task.await.unwrap().unwrap();
    journal.close().await.unwrap();
}

#[tokio::test]
async fn sdk_discovery_is_the_shared_catalog_and_invalid_calls_have_structured_errors() {
    let principal = Principal::new("host").unwrap();
    let policy = PolicyStore::default();
    policy
        .replace(
            &principal,
            "demo",
            Grant::new(0, Budget::new("budget").unwrap()),
        )
        .unwrap();
    let session = ToolSession::new(
        Executor::new(policy, ExecutorOptions::default()).unwrap(),
        principal,
    );
    let server = McpServer::new(session, ExecutionMode::Execute, OutputLimit::default());
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server_task = tokio::spawn(async move { server.serve(server_io).await });
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 8);
    for (actual, expected) in tools.iter().zip(catalog().tools) {
        assert_eq!(actual.name, expected.name);
        assert_eq!(
            Value::Object((*actual.input_schema).clone()),
            expected.input_schema
        );
        assert_eq!(
            Value::Object((**actual.output_schema.as_ref().unwrap()).clone()),
            expected.output_schema
        );
        assert_eq!(
            serde_json::to_value(actual.annotations.as_ref().unwrap()).unwrap(),
            serde_json::to_value(expected.annotations).unwrap()
        );
    }
    let result = client
        .call_tool(
            CallToolRequestParams::new("send_message")
                .with_arguments(json!({"token":"PRIVATE"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.as_ref().unwrap()["error"]["code"],
        "executor_invalid_input"
    );
    assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE"));
    client.cancel().await.unwrap();
    server_task.await.unwrap().unwrap();
}
