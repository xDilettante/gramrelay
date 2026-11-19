#![cfg(feature = "cli")]
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
const TOKEN: &str = "0:OFFLINE_CLI_TOKEN";
const TOKEN_ENV: &str = "TG_CLI_TEST_TOKEN";
struct Fixture {
    directory: tempfile::TempDir,
    server: MockServer,
    config: Value,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let config = json!({"schema_version":1,"principal":"host","journal_path":directory.path().join("journal.sqlite"),"profiles":[{"name":"demo","bot_id":"0","token_env":TOKEN_ENV,"api_url":server.uri(),"allow_http_loopback":true,"grant":{"actions":["get_me","get_chat","read_updates","send_message","edit_message_text","answer_callback_query","get_operation","send_document"],"chats":["1"],"budget":{"id":"stable-budget"}}}]});
        Self {
            directory,
            server,
            config,
        }
    }
    async fn run(&self, command: &str, input: &[u8], dry_run: bool) -> std::process::Output {
        self.run_config(
            &serde_json::to_vec(&self.config).unwrap(),
            command,
            input,
            dry_run,
            true,
        )
        .await
    }
    async fn run_config(
        &self,
        bytes: &[u8],
        command: &str,
        input: &[u8],
        dry_run: bool,
        token: bool,
    ) -> std::process::Output {
        let config = self.directory.path().join("config.json");
        std::fs::write(&config, bytes).unwrap();
        let mut command_line = Command::new(env!("CARGO_BIN_EXE_tg-agent"));
        command_line
            .arg(command)
            .arg("--config")
            .arg(&config)
            .env(TOKEN_ENV, TOKEN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !token {
            command_line.env_remove(TOKEN_ENV);
        }
        if dry_run {
            command_line.arg("--dry-run");
        }
        let mut child = command_line.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(input).await;
        drop(stdin);
        tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .expect("CLI process must finish")
            .unwrap()
    }
    async fn replies(&self) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/getMe")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
            ))
            .expect(1)
            .mount(&self.server)
            .await;
        Mock::given(method("POST")).and(path(format!("/bot{TOKEN}/sendMessage"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_REMOTE_TEXT"}}))).expect(1).mount(&self.server).await;
    }
}
fn send() -> Value {
    json!({"schema_version":1,"profile":"demo","operation_id":"same-key","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"Hello"}}})
}
fn decode(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("CLI stdout must contain JSON")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_or_inconsistent_host_fields_fail_closed_without_storage_or_secret_output() {
    let mutations = [
        ("/schema_version", json!(2)),
        ("/principal", json!("not a principal")),
        ("/journal_path", json!("PRIVATE_RELATIVE")),
        ("/profiles/0/bot_id", json!("00")),
        ("/profiles/0/bot_id", json!(0)),
        ("/profiles/0/token_env", json!("PRIVATE=INVALID")),
        ("/profiles/0/token", json!("PRIVATE_TOKEN")),
        ("/profiles/0/grant/approval", json!(true)),
        (
            "/profiles/0/grant/actions",
            json!(["read_updates", "read_updates"]),
        ),
        ("/profiles/0/grant/all_chats", json!(true)),
        (
            "/profiles/0/grant/require_approval",
            json!(["delete_message"]),
        ),
        ("/profiles/0/grant/files", json!(["C:/PRIVATE_FILE"])),
        ("/profiles/0/grant/budget/window_seconds", json!(0)),
        ("/profiles/0/grant/budget/limits", json!({"invocations":-1})),
        (
            "/profiles/0/grant/costs",
            json!([{"action":"send_message","units":9223372036854775808u64}]),
        ),
    ];
    for (pointer, value) in mutations {
        let mut fixture = Fixture::new().await;
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        fixture.config.pointer_mut(parent).unwrap()[key] = value;
        let output = fixture
            .run("run", send().to_string().as_bytes(), false)
            .await;
        assert_eq!(
            output.status.code(),
            Some(2),
            "{pointer}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(decode(&output)["error"]["code"], "agent_config");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        assert!(output.stderr.is_empty());
        assert!(!fixture.directory.path().join("journal.sqlite").exists());
        assert!(fixture.server.received_requests().await.unwrap().is_empty());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_fields_positional_arrays_and_oversized_config_never_initialize_storage() {
    let fixture = Fixture::new().await;
    let encoded = fixture.config.to_string();
    let duplicate = encoded.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    let positional = json!([
        1,
        "host",
        fixture.directory.path().join("journal.sqlite"),
        []
    ])
    .to_string();
    let profile_array =
        encoded.replacen(fixture.config["profiles"][0].to_string().as_str(), "[]", 1);
    for bytes in [
        duplicate.into_bytes(),
        positional.into_bytes(),
        profile_array.into_bytes(),
        vec![b' '; 1024 * 1024 + 1],
    ] {
        let output = fixture
            .run_config(&bytes, "run", send().to_string().as_bytes(), false, true)
            .await;
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(decode(&output)["error"]["code"], "agent_config");
        assert!(!fixture.directory.path().join("journal.sqlite").exists());
    }
    let output = fixture
        .run_config(
            encoded.as_bytes(),
            "run",
            send().to_string().as_bytes(),
            false,
            false,
        )
        .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.directory.path().join("journal.sqlite").exists());
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_and_oversized_input_do_not_create_operation_attempts_or_send_rpc() {
    let fixture = Fixture::new().await;
    let output = fixture
        .run("run", send().to_string().as_bytes(), true)
        .await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(decode(&output)["dry_run"]["is_write"], true);
    let output = fixture.run("serve", &vec![b'x'; 65537], false).await;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(decode(&output)["error"]["code"], "agent_input_too_large");
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("journal.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_operator_approval_cannot_authorize_a_changed_key_or_payload() {
    let mut fixture = Fixture::new().await;
    fixture.replies().await;
    fixture.config["profiles"][0]["grant"]["require_approval"] = json!(["send_message"]);
    fixture.config["approvals"] = json!([{"request":send(),"expires_at_ms":now()+60000}]);
    let mut changed = send();
    changed["operation_id"] = json!("new-key");
    let mut payload = send();
    payload["call"]["arguments"]["text"] = json!("different");
    for request in [changed, payload] {
        let output = fixture
            .run("run", request.to_string().as_bytes(), false)
            .await;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            decode(&output)["error"]["code"],
            "executor_approval_required"
        );
    }
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    let output = fixture
        .run("run", send().to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(decode(&output)["result"]["operation"]["state"], "succeeded");
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_remote_result_is_retained_across_processes_without_replay() {
    let fixture = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ))
        .expect(1)
        .mount(&fixture.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/sendMessage")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("PRIVATE_MALFORMED_RESPONSE", "application/json"),
        )
        .expect(1)
        .mount(&fixture.server)
        .await;
    for _ in 0..2 {
        let output = fixture
            .run("run", send().to_string().as_bytes(), false)
            .await;
        assert_eq!(output.status.code(), Some(0));
        let value = decode(&output);
        assert_eq!(value["result"]["operation"]["state"], "unknown");
        assert_eq!(
            value["result"]["operation"]["failure"]["outcome"],
            "unknown"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
async fn durable_inputs(fixture: &Fixture) -> (String, String) {
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
    feed.append_webhook(&source,serde_json::from_value(json!({"update_id":1,"callback_query":{"id":"observed-query","from":{"id":1,"is_bot":false,"first_name":"Demo"},"chat_instance":"demo","message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}})).unwrap()).await.unwrap();
    feed.release_source(source).await.unwrap();
    let cursor = feed.read(None, 1).await.unwrap().events()[0]
        .cursor()
        .to_string();
    journal.close().await.unwrap();
    (file.handle().to_owned(), cursor)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_eight_actions_execute_in_a_real_process_with_host_files_and_approvals() {
    use tg_agent::executor::Action;
    let mut fixture = Fixture::new().await;
    let (handle, _) = durable_inputs(&fixture).await;
    let source = fixture.directory.path().join("restored-file.bin");
    std::fs::write(&source, [0, 255, 3]).unwrap();
    fixture.config["file_restorations"] =
        json!([{"profile":"demo","file_handle":handle,"path":source}]);
    fixture.config["callback_clock_policy"] = json!("trusted_wall_clock");
    fixture.config["profiles"][0]["grant"]["files"] = json!([handle]);
    fixture.config["profiles"][0]["grant"]["require_approval"] =
        json!(["send_message", "send_document"]);
    let catalog = tg_agent::tools::catalog();
    let mut requests = Vec::new();
    let mut approvals = Vec::new();
    for (action, tool) in Action::ALL.into_iter().zip(&catalog.tools) {
        let mut request = tool.examples[0]["input"].clone();
        if action.is_write() {
            request["operation_id"] = json!(format!("op-{}", action.name()));
        }
        match action {
            Action::GetOperation => {
                request["call"]["arguments"]["operation_id"] = json!("op-send_message")
            }
            Action::AnswerCallbackQuery => {
                request["call"]["arguments"]["callback_query_id"] = json!("observed-query")
            }
            Action::SendDocument => request["call"]["arguments"]["file_handle"] = json!(handle),
            _ => {}
        }
        if matches!(action, Action::SendMessage | Action::SendDocument) {
            approvals.push(json!({"request":request,"expires_at_ms":now()+30000}));
        }
        requests.push(request.to_string());
    }
    fixture.config["approvals"] = json!(approvals);
    let chat = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(1),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    );
    let message =
        json!({"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_RESPONSE"});
    for (name, result) in [
        (
            "getMe",
            json!({"id":0,"is_bot":true,"first_name":"Offline"}),
        ),
        ("getChat", serde_json::to_value(chat).unwrap()),
        ("sendMessage", message.clone()),
        ("editMessageText", message.clone()),
        ("answerCallbackQuery", json!(true)),
        ("sendDocument", message),
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/{name}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":result})),
            )
            .expect(1)
            .mount(&fixture.server)
            .await;
    }
    let input = requests.join("\n") + "\n";
    let output = fixture.run("serve", input.as_bytes(), false).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let frames = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 8);
    for (tool, frame) in catalog.tools.iter().zip(&frames) {
        assert_eq!(frame["ok"], true, "{}: {frame}", tool.name);
        assert!(
            jsonschema::validator_for(&tool.output_schema)
                .unwrap()
                .is_valid(frame),
            "{} schema",
            tool.name
        );
        assert!(!frame.to_string().contains("PRIVATE_RESPONSE"));
    }
    assert_eq!(
        frames[2]["result"]["updates"]["events"][0]["untrusted"],
        true
    );
    let requests = fixture.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 6);
    let upload = requests
        .iter()
        .find(|request| request.url.path().ends_with("/sendDocument"))
        .unwrap();
    assert!(upload.body.windows(3).any(|bytes| bytes == [0, 255, 3]));
    assert!(String::from_utf8_lossy(&upload.body).contains("filename=\"PRIVATE.bin\""));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restoration_requires_exact_bytes_and_cached_document_needs_no_restoration() {
    let mut fixture = Fixture::new().await;
    let (handle, _) = durable_inputs(&fixture).await;
    let source = fixture.directory.path().join("file.bin");
    std::fs::write(&source, [0, 254, 3]).unwrap();
    fixture.config["file_restorations"] =
        json!([{"profile":"demo","file_handle":handle,"path":source}]);
    fixture.config["profiles"][0]["grant"]["files"] = json!([handle]);
    let request = json!({"schema_version":1,"profile":"demo","operation_id":"document-key","call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":handle}}});
    let output = fixture
        .run("run", request.to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    std::fs::write(&source, [0, 255, 3]).unwrap();
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ))
        .expect(1)
        .mount(&fixture.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/sendDocument")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}),
        ))
        .expect(1)
        .mount(&fixture.server)
        .await;
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    fixture.config["file_restorations"] = json!([]);
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    fixture.config["profiles"][0]["grant"]["files"] = json!([]);
    let output = fixture
        .run("run", request.to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(decode(&output)["error"]["code"], "executor_denied");
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_update_options_open_an_existing_feed_without_changing_its_retention() {
    use tg_agent::{
        journal::{Journal, JournalOptions},
        updates::{UpdateFeed, UpdateOptions},
    };
    let mut fixture = Fixture::new().await;
    let database = fixture.directory.path().join("journal.sqlite");
    let journal = Journal::open(&database, JournalOptions::default())
        .await
        .unwrap();
    UpdateFeed::open(
        journal.clone(),
        "demo",
        0,
        UpdateOptions::default().retention(Duration::from_secs(86400)),
    )
    .await
    .unwrap();
    journal.close().await.unwrap();
    fixture.config["profiles"][0]["update_options"] = json!({"retention_seconds":86400});
    let request =
        json!({"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{}}});
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    fixture.config["profiles"][0]["update_options"]["retention_seconds"] = json!(172800);
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(2)
    );
    let connection = rusqlite::Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT retention_ms FROM update_feeds", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        86400000
    );
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_stdout_after_dispatch_keeps_the_durable_receipt_for_the_next_process() {
    let fixture = Fixture::new().await;
    fixture.replies().await;
    let config = fixture.directory.path().join("config.json");
    std::fs::write(&config, fixture.config.to_string()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tg-agent"))
        .arg("run")
        .arg("--config")
        .arg(&config)
        .env(TOKEN_ENV, TOKEN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(send().to_string().as_bytes())
        .await
        .unwrap();
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), "agent_io");
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("journal.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT state FROM operations WHERE operation_id='same-key'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "succeeded"
    );
    assert_eq!(
        fixture
            .run("run", send().to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callback_observation_needs_explicit_clock_scope_and_the_exact_retained_record() {
    use tg_agent::{
        journal::{Journal, JournalOptions},
        updates::{SourceMode, UpdateFeed, UpdateOptions},
    };
    let mut fixture = Fixture::new().await;
    let (_, cursor) = durable_inputs(&fixture).await;
    fixture.config["callback_observations"] = json!([{"profile":"demo","cursor":cursor}]);
    let request = json!({"schema_version":1,"profile":"demo","operation_id":"callback-key","call":{"tool":"answer_callback_query","arguments":{"callback_query_id":"observed-query"}}});
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(2)
    );
    fixture.config["callback_clock_policy"] = json!("trusted_wall_clock");
    fixture.config["profiles"][0]["grant"]["chats"] = json!(["2"]);
    assert_eq!(
        fixture
            .run("run", request.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(2)
    );
    fixture.config["profiles"][0]["grant"]["chats"] = json!(["1"]);
    let database = fixture.directory.path().join("journal.sqlite");
    let journal = Journal::open(&database, JournalOptions::default())
        .await
        .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source,serde_json::from_value(json!({"update_id":2,"callback_query":{"id":"next-query","from":{"id":1,"is_bot":false,"first_name":"Demo"},"chat_instance":"demo","message":{"message_id":3,"date":1,"chat":{"id":1,"type":"private"}}}})).unwrap()).await.unwrap();
    feed.release_source(source).await.unwrap();
    journal.close().await.unwrap();
    // A deliberately missing position must not resolve to the following callback.
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute("DELETE FROM updates WHERE sequence=1", [])
        .unwrap();
    let mut next = request;
    next["call"]["arguments"]["callback_query_id"] = json!("next-query");
    assert_eq!(
        fixture
            .run("run", next.to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(2)
    );
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_quota_survives_process_restart_and_new_caps_apply_to_retained_usage() {
    let mut fixture = Fixture::new().await;
    fixture.replies().await;
    fixture.config["profiles"][0]["grant"]["budget"]["window_seconds"] = json!(86400);
    fixture.config["profiles"][0]["grant"]["budget"]["limits"] = json!({"invocations":1});
    assert_eq!(
        fixture
            .run("run", send().to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    let output = fixture
        .run("run", send().to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(decode(&output)["error"]["code"], "executor_quota_exceeded");
    fixture.config["profiles"][0]["grant"]["budget"]["limits"]["invocations"] = json!(3);
    assert_eq!(
        fixture
            .run("run", send().to_string().as_bytes(), false)
            .await
            .status
            .code(),
        Some(0)
    );
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("journal.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT invocations,write_attempts FROM executor_budgets",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (2, 1)
    );
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_approval_cursor_is_rejected_before_storage_is_opened() {
    let mut fixture = Fixture::new().await;
    fixture.config["approvals"] = json!([{"request":{"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{"cursor":"garbage"}}},"expires_at_ms":now()+60000}]);
    let output = fixture
        .run("run", send().to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !fixture.directory.path().join("journal.sqlite").exists(),
        "cursor syntax must be checked before opening storage"
    );
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn statically_invalid_confirmations_fail_before_creating_a_journal() {
    for invalid_expiry in [false, true] {
        let mut fixture = Fixture::new().await;
        let mut request = send();
        let expires = now() + 20000;
        if invalid_expiry {
            fixture.config["profiles"][0]["grant"]["expires_at_ms"] = json!(expires - 10000);
        } else {
            request["call"]["arguments"]["chat_id"] = json!("2");
        }
        fixture.config["approvals"] = json!([{"request":request,"expires_at_ms":expires}]);
        let output = fixture
            .run("run", send().to_string().as_bytes(), false)
            .await;
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(decode(&output)["error"]["code"], "agent_config");
        assert!(
            !fixture.directory.path().join("journal.sqlite").exists(),
            "invalid static confirmation must not initialize the journal"
        );
        assert!(fixture.server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_bot_identity_must_match_the_environment_token_before_journal_open() {
    let mut fixture = Fixture::new().await;
    fixture.config["profiles"][0]["bot_id"] = json!("1");
    let input =
        json!({"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{}}});
    let output = fixture
        .run("run", input.to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(decode(&output)["error"]["code"], "agent_config");
    assert!(!fixture.directory.path().join("journal.sqlite").exists());
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_shot_callback_uses_explicit_retained_source_and_exact_host_approval() {
    use tg_agent::{
        journal::{Journal, JournalOptions},
        updates::{SourceMode, UpdateFeed, UpdateOptions},
    };
    let mut fixture = Fixture::new().await;
    let journal = Journal::open(
        fixture.directory.path().join("journal.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    let event=serde_json::from_value(json!({"update_id":1,"callback_query":{"id":"observed-query","from":{"id":1,"is_bot":false,"first_name":"Demo"},"chat_instance":"demo","message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}})).unwrap();
    feed.append_webhook(&source, event).await.unwrap();
    feed.release_source(source).await.unwrap();
    let cursor = feed.read(None, 1).await.unwrap().events()[0]
        .cursor()
        .to_string();
    journal.close().await.unwrap();
    let request = json!({"schema_version":1,"profile":"demo","operation_id":"callback-key","call":{"tool":"answer_callback_query","arguments":{"callback_query_id":"observed-query","text":"Accepted"}}});
    fixture.config["callback_clock_policy"] = json!("trusted_wall_clock");
    fixture.config["callback_observations"] = json!([{"profile":"demo","cursor":cursor}]);
    fixture.config["profiles"][0]["grant"]["require_approval"] = json!(["answer_callback_query"]);
    fixture.config["approvals"] = json!([{"request":request,"expires_at_ms":now()+5000}]);
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
        ))
        .expect(1)
        .mount(&fixture.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/answerCallbackQuery")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":true})))
        .expect(1)
        .mount(&fixture.server)
        .await;
    let output = fixture
        .run("run", request.to_string().as_bytes(), false)
        .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(decode(&output)["result"]["operation"]["acknowledged"], true);
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_local_read_uses_host_policy_without_identity_or_polling() {
    let fixture = Fixture::new().await;
    let request = json!({"schema_version":1,"profile":"demo","call":{"tool":"read_updates","arguments":{"limit":1}}});
    let output = fixture
        .run("run", request.to_string().as_bytes(), false)
        .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value = decode(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["updates"]["events"], json!([]));
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_same_key_survives_process_restart_and_current_scope_controls_disclosure() {
    let mut fixture = Fixture::new().await;
    fixture.replies().await;
    for _ in 0..2 {
        let output = fixture
            .run("run", send().to_string().as_bytes(), false)
            .await;
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value = decode(&output);
        assert_eq!(value["result"]["operation"]["state"], "succeeded");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_REMOTE_TEXT"));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    fixture.config["profiles"][0]["grant"]["chats"] = json!(["2"]);
    let output = fixture
        .run("run", send().to_string().as_bytes(), false)
        .await;
    assert_eq!(output.status.code(), Some(1));
    let value = decode(&output);
    assert_eq!(value["error"]["code"], "executor_denied");
    assert!(value.get("result").is_none());
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_serve_recovers_from_bad_frames_without_repeating_a_write() {
    let fixture = Fixture::new().await;
    fixture.replies().await;
    let input = format!(
        "{{\"principal\":\"PRIVATE_INJECTION\"}}\n{}\n{}\n",
        send(),
        send()
    );
    let output = fixture.run("serve", input.as_bytes(), false).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let frames = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["error"]["code"], "executor_invalid_input");
    assert!(!frames[0].to_string().contains("PRIVATE"));
    assert_eq!(
        frames[1]["result"]["operation"]["state"], "succeeded",
        "{frames:?}"
    );
    assert_eq!(frames[1], frames[2]);
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
}
