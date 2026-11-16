#![cfg(feature = "mcp")]
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
const TOKEN: &str = "0:OFFLINE_MCP_TOKEN";
const TOKEN_ENV: &str = "TG_MCP_TEST_TOKEN";
struct Fixture {
    directory: tempfile::TempDir,
    server: MockServer,
    config: Value,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let config = json!({"schema_version":1,"principal":"host","journal_path":directory.path().join("journal.sqlite"),"profiles":[{"name":"demo","bot_id":"0","token_env":TOKEN_ENV,"api_url":server.uri(),"allow_http_loopback":true,"grant":{"actions":tg_agent::executor::Action::ALL.map(|action| action.name()),"chats":["1"],"budget":{"id":"stable-budget"}}}]});
        Self {
            directory,
            server,
            config,
        }
    }
    async fn start(&self, dry_run: bool) -> Client {
        let config_path = self.directory.path().join("config.json");
        std::fs::write(&config_path, self.config.to_string()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tg-agent"));
        command
            .arg("mcp")
            .arg("--config")
            .arg(config_path)
            .env(TOKEN_ENV, TOKEN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if dry_run {
            command.arg("--dry-run");
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Client {
            child,
            input,
            output,
            id: 0,
        };
        let initialized = client.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"offline-test","version":"1"}})).await;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(initialized["result"]["capabilities"], json!({"tools":{}}));
        client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        client
    }
    async fn reply(&self, name: &str, result: Value, delay: Duration) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/{name}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(delay)
                    .set_body_json(json!({"ok":true,"result":result})),
            )
            .mount(&self.server)
            .await;
    }
    async fn identity(&self) {
        self.reply(
            "getMe",
            json!({"id":0,"is_bot":true,"first_name":"Offline"}),
            Duration::ZERO,
        )
        .await;
    }
}
struct Client {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    id: u64,
}
impl Client {
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
        self.input.flush().await.unwrap();
    }
    async fn receive(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.output.next_line())
            .await
            .expect("MCP response timeout")
            .unwrap()
            .expect("MCP response before EOF");
        serde_json::from_str(&line).expect("stdout exclusively JSON-RPC")
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        let result = self.receive().await;
        assert_eq!(result["id"], id);
        result
    }
    async fn call(&mut self, tool: &str, request: Value) -> Value {
        let wire = self
            .request("tools/call", json!({"name":tool,"arguments":request}))
            .await;
        let result = &wire["result"];
        assert!(result.is_object(), "{wire}");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
        text
    }
    async fn finish(self, code: i32) {
        let Self {
            child,
            input,
            mut output,
            ..
        } = self;
        drop(input);
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = output.next_line().await.unwrap() {
                let _: Value =
                    serde_json::from_str(&line).expect("drained stdout remains JSON-RPC");
            }
        })
        .await
        .expect("MCP shutdown must finish");
        drop(output);
        let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 0 {
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
fn request(tool: &str, arguments: Value) -> Value {
    let mut value =
        json!({"schema_version":1,"profile":"demo","call":{"tool":tool,"arguments":arguments}});
    if tg_agent::executor::Action::ALL
        .into_iter()
        .any(|action| action.name() == tool && action.is_write())
    {
        value["operation_id"] = json!(format!("op-{tool}"));
    }
    value
}
fn send() -> Value {
    request("send_message", json!({"chat_id":"1","text":"Hello"}))
}
fn message() -> Value {
    json!({"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_RESPONSE"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_stdio_legacy_discovery_dry_run_and_scope_binding_never_make_remote_calls() {
    let fixture = Fixture::new().await;
    let mut client = fixture.start(true).await;
    let tools = client.request("tools/list", json!({})).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 8);
    assert!(tools["result"].get("nextCursor").is_none());
    let value = client.call("send_message", send()).await;
    assert_eq!(value["dry_run"]["action"], "send_message");
    let mismatch = client.call("get_me", send()).await;
    assert_eq!(mismatch["error"]["code"], "agent_tool_mismatch");
    let denied = client
        .call(
            "send_message",
            request("send_message", json!({"chat_id":"2","text":"Forbidden"})),
        )
        .await;
    assert_eq!(denied["ok"], false);
    let mut injected = send();
    injected["approval"] = json!(true);
    assert_eq!(
        client.call("send_message", injected).await["error"]["code"],
        "executor_invalid_input"
    );
    let unknown = client
        .request(
            "tools/call",
            json!({"name":"PRIVATE_UNKNOWN","arguments":{}}),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(!unknown.to_string().contains("PRIVATE"));
    assert!(
        client
            .request("tools/list", json!({"cursor":"PRIVATE"}))
            .await["error"]
            .is_object()
    );
    client.finish(0).await;
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_stdio_durable_same_key_and_unknown_receipts_survive_restart_without_replay() {
    for unknown in [false, true] {
        let fixture = Fixture::new().await;
        fixture.identity().await;
        fixture
            .reply(
                "sendMessage",
                if unknown {
                    json!({"unexpected":"PRIVATE_REMOTE"})
                } else {
                    message()
                },
                Duration::ZERO,
            )
            .await;
        for _ in 0..2 {
            let mut client = fixture.start(false).await;
            let first = client.call("send_message", send()).await;
            let again = client.call("send_message", send()).await;
            assert_eq!(first, again);
            assert_eq!(
                first["result"]["operation"]["state"],
                if unknown { "unknown" } else { "succeeded" }
            );
            let status = client
                .call(
                    "get_operation",
                    request("get_operation", json!({"operation_id":"op-send_message"})),
                )
                .await;
            assert_eq!(status["result"]["operation"], first["result"]["operation"]);
            assert!(!first.to_string().contains("PRIVATE"));
            client.finish(0).await;
        }
        assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
async fn durable_inputs(fixture: &Fixture) -> String {
    use tg_agent::{
        executor::{Executor, ExecutorOptions, PolicyStore, Principal},
        journal::{Journal, JournalOptions},
        updates::{SourceMode, UpdateFeed, UpdateOptions},
    };
    let journal = Journal::open(
        fixture.directory.path().join("journal.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
        .await
        .unwrap();
    let executor = Executor::new(PolicyStore::default(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            feed.clone(),
            tg::test_kit::MockTransport::new([]).unwrap().bot().unwrap(),
        )
        .unwrap();
    let file = executor
        .register_file(
            &Principal::new("host").unwrap(),
            "demo",
            "PRIVATE.bin",
            vec![0, 255, 3],
            now() + 60000,
        )
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source, serde_json::from_value(json!({"update_id":1,"callback_query":{"id":"observed-query","from":{"id":1,"is_bot":false,"first_name":"Demo"},"chat_instance":"demo","message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}})).unwrap()).await.unwrap();
    feed.release_source(source).await.unwrap();
    journal.close().await.unwrap();
    file.handle().to_owned()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_stdio_all_eight_tools_share_approvals_callback_provenance_and_file_ownership() {
    use tg_agent::executor::Action;
    let mut fixture = Fixture::new().await;
    let handle = durable_inputs(&fixture).await;
    let file_path = fixture.directory.path().join("document.bin");
    std::fs::write(&file_path, [0, 255, 3]).unwrap();
    fixture.config["file_restorations"] =
        json!([{"profile":"demo","file_handle":handle,"path":file_path}]);
    fixture.config["callback_clock_policy"] = json!("trusted_wall_clock");
    fixture.config["profiles"][0]["grant"]["files"] = json!([handle]);
    fixture.config["profiles"][0]["grant"]["require_approval"] =
        json!(["send_message", "send_document"]);
    let mut requests = Vec::new();
    let mut approvals = Vec::new();
    for (action, tool) in Action::ALL
        .into_iter()
        .zip(tg_agent::tools::catalog().tools)
    {
        let mut input = tool.examples[0]["input"].clone();
        if action.is_write() {
            input["operation_id"] = json!(format!("op-{}", action.name()));
        }
        match action {
            Action::GetOperation => {
                input["call"]["arguments"]["operation_id"] = json!("op-send_message")
            }
            Action::AnswerCallbackQuery => {
                input["call"]["arguments"]["callback_query_id"] = json!("observed-query")
            }
            Action::SendDocument => input["call"]["arguments"]["file_handle"] = json!(handle),
            _ => {}
        }
        if matches!(action, Action::SendMessage | Action::SendDocument) {
            approvals.push(json!({"request":input,"expires_at_ms":now()+30000}));
        }
        requests.push((action, input, tool.output_schema));
    }
    fixture.config["approvals"] = json!(approvals);
    fixture.identity().await;
    let chat = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(1),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    );
    for (name, value) in [
        ("getChat", serde_json::to_value(chat).unwrap()),
        ("sendMessage", message()),
        ("editMessageText", message()),
        ("answerCallbackQuery", json!(true)),
        ("sendDocument", message()),
    ] {
        fixture.reply(name, value, Duration::ZERO).await;
    }
    let mut client = fixture.start(false).await;
    // Approval cannot be supplied by a model or inferred from annotations.
    let mut unapproved = send();
    unapproved["operation_id"] = json!("different-key");
    assert_eq!(client.call("send_message", unapproved).await["ok"], false);
    for (action, input, schema) in requests {
        let value = client.call(action.name(), input).await;
        assert_eq!(value["ok"], true, "{}: {value}", action.name());
        assert!(
            jsonschema::validator_for(&schema).unwrap().is_valid(&value),
            "{}: {value}",
            action.name()
        );
        if action.is_write() {
            assert_eq!(
                value["result"]["operation"]["state"], "succeeded",
                "{value}"
            );
        }
    }
    client.finish(0).await;
    let remote = fixture.server.received_requests().await.unwrap();
    assert_eq!(remote.len(), 6);
    assert!(
        remote
            .iter()
            .any(|request| request.url.path().ends_with("sendDocument")
                && request.body.windows(3).any(|bytes| bytes == [0, 255, 3]))
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_eof_after_remote_admission_never_replay_the_write() {
    for cancel in [false, true] {
        let fixture = Fixture::new().await;
        fixture.identity().await;
        fixture
            .reply("sendMessage", message(), Duration::from_secs(30))
            .await;
        let mut client = fixture.start(false).await;
        client.send(json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"send_message","arguments":send()}})).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if fixture
                    .server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .any(|request| request.url.path().ends_with("sendMessage"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if cancel {
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":100}})).await;
            // A subsequent metadata request remains usable; no cancelled result
            // may accidentally be emitted as this request's response.
            assert!(client.request("tools/list", json!({})).await["result"].is_object());
        }
        client.finish(0).await;
        let mut recovered = fixture.start(false).await;
        let status = recovered
            .call(
                "get_operation",
                request("get_operation", json!({"operation_id":"op-send_message"})),
            )
            .await;
        let state = &status["result"]["operation"]["state"];
        assert!(state == "in_flight" || state == "unknown", "{status}");
        // Before the persisted lease expires, in_flight is deliberately kept:
        // neither a new process nor MCP cancellation may infer non-delivery.
        let repeated = recovered.call("send_message", send()).await;
        assert!(
            repeated["ok"] == false || repeated["result"]["operation"]["state"] == *state,
            "{repeated}"
        );
        recovered.finish(0).await;
        assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_failure_writes_no_non_protocol_stdout_or_private_diagnostics() {
    let fixture = Fixture::new().await;
    let output = Command::new(env!("CARGO_BIN_EXE_tg-agent"))
        .arg("mcp")
        .arg("--config")
        .arg(fixture.directory.path().join("PRIVATE_MISSING.json"))
        .env_remove(TOKEN_ENV)
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "agent_config"
    );
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_stdout_after_remote_admission_fails_transport_and_preserves_the_original_key() {
    let fixture = Fixture::new().await;
    fixture.identity().await;
    fixture
        .reply("sendMessage", message(), Duration::from_secs(30))
        .await;
    let mut client = fixture.start(false).await;
    client.send(json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"send_message","arguments":send()}})).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture
                .server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| request.url.path().ends_with("sendMessage"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let Client {
        child,
        input,
        output,
        ..
    } = client;
    drop(output);
    drop(input);
    let finished = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(finished.stderr).unwrap().trim(),
        "agent_mcp_transport"
    );
    let mut recovered = fixture.start(false).await;
    let status = recovered
        .call(
            "get_operation",
            request("get_operation", json!({"operation_id":"op-send_message"})),
        )
        .await;
    let state = &status["result"]["operation"]["state"];
    assert!(state == "in_flight" || state == "unknown", "{status}");
    let repeated = recovered.call("send_message", send()).await;
    assert!(
        repeated["ok"] == false || repeated["result"]["operation"]["state"] == *state,
        "{repeated}"
    );
    recovered.finish(0).await;
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
