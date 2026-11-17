#![cfg(all(feature = "cli", feature = "mtproto-test-kit"))]
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, ClientOptions, Session, SessionBinding, SessionKey,
    SessionOptions, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};
use tokio::{io::AsyncWriteExt, process::Command};
#[path = "../src/bin/fixtures/mtproto_wire.rs"]
mod wire;
const KEY_ENV: &str = "TG_MT_HOST_TEST_KEY";
fn request(tool: &str, args: Value, key: Option<&str>) -> Value {
    let mut v =
        json!({"schema_version":1,"profile":"personal","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        v["operation_id"] = json!(key);
    }
    v
}
async fn provision(path: &std::path::Path, id: i64) {
    let session = Session::open(
        path,
        SessionBinding::new("synthetic", 1).unwrap(),
        SessionKey::from_bytes([7; 32]),
        SessionOptions::default(),
    )
    .await
    .unwrap();
    let client = test_kit::client(
        session,
        ClientOptions::default(),
        move |_, bytes| async move {
            assert_eq!(
                u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                tl::functions::users::GetUsers::CONSTRUCTOR_ID
            );
            let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
            v.extend_from_slice(&((1u32 << 10) | (1 << 14)).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&id.to_le_bytes());
            v.extend_from_slice(&0i32.to_le_bytes());
            Ok(vec![tl::enums::User::from_bytes(&v).unwrap()].to_bytes())
        },
    )
    .unwrap();
    client
        .verify_account(AccountIdentity {
            id: AccountId::new(id).unwrap(),
            kind: AccountKind::Bot,
        })
        .await
        .unwrap();
    client.close().await.unwrap();
}
struct Fixture {
    dir: tempfile::TempDir,
    config: Value,
    listener: tokio::net::TcpListener,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        provision(&dir.path().join("session.sqlite"), 42).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let std::net::SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        let session = Session::open_existing(
            dir.path().join("session.sqlite"),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .unwrap();
        test_kit::route_loopback(&session, endpoint).await.unwrap();
        session.close().await.unwrap();
        let config = json!({"schema_version":2,"principal":"host","journal_path":dir.path().join("agent.sqlite"),"profiles":[],"mtproto":{"profiles":[{"name":"personal","account_kind":"bot","account_id":"42","session_path":dir.path().join("session.sqlite"),"session_binding":"synthetic","api_id":1,"session_key_env":KEY_ENV,"grant":{"actions":["mtproto_get_me","mtproto_get_peer","mtproto_read_updates","mtproto_send_message","mtproto_edit_message_text","mtproto_answer_callback_query","mtproto_get_operation","mtproto_send_document"],"peers":[{"kind":"user","id":"42"}],"budget":{"id":"daily"}}}]}});
        let mut config = config;
        config["mtproto"]["profiles"][0]["request_timeout_ms"] = json!(30000);
        config["mtproto"]["profiles"][0]["max_pending"] = json!(16);
        Self {
            dir,
            config,
            listener,
        }
    }
    async fn run(
        &self,
        config: &Value,
        command: &str,
        input: &Value,
        dry: bool,
        key: Option<&str>,
    ) -> std::process::Output {
        let path = self.dir.path().join("host.json");
        std::fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tg-agent"));
        cmd.arg(command)
            .arg("--config")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd.env("TG_MT_HOST_BOT_TOKEN", "0:OFFLINE_MIXED");
        if let Some(key) = key {
            cmd.env(KEY_ENV, key);
        } else {
            cmd.env_remove(KEY_ENV);
        }
        if dry {
            cmd.arg("--dry-run");
        }
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&serde_json::to_vec(input).unwrap())
            .await
            .unwrap();
        drop(stdin);
        tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    }
    async fn reusable(&self, id: i64, path: &std::path::Path) {
        let s = Session::open_existing(
            path,
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(s.remembered_account().unwrap().unwrap().id.get(), id);
        s.close().await.unwrap();
    }
    async fn no_connections(&self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(30), self.listener.accept())
                .await
                .is_err(),
            "ordinary host made a network connection"
        );
    }
    async fn seed(&mut self) -> Vec<Value> {
        use tg_agent::{
            journal::{Journal, JournalOptions},
            mtproto::{
                Action, Budget, Executor, ExecutorOptions, Grant, PeerId, PeerKind, PeerScope,
                PolicyStore, Principal,
                updates::{Feed, Options},
            },
            profiles::ProfileIdentity,
        };
        let journal = Journal::open(
            self.dir.path().join("agent.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let feed = Feed::open(
            journal.clone(),
            "personal",
            ProfileIdentity::mtproto_bot(42).unwrap(),
            Options::default(),
        )
        .await
        .unwrap();
        let stored = Session::open_existing(
            self.dir.path().join("session.sqlite"),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .unwrap();
        let identity = AccountIdentity {
            id: AccountId::new(42).unwrap(),
            kind: AccountKind::Bot,
        };
        let client = test_kit::client(
            stored,
            ClientOptions::default(),
            move |_, bytes| async move {
                assert!(!wire::is_write(&bytes));
                Ok(wire::response(identity, &bytes))
            },
        )
        .unwrap();
        let mut source = feed.claim_source().await.unwrap();
        let mut receiver = client
            .account(identity)
            .updates(tg_mtproto::UpdateMode::StartNow)
            .unwrap();
        for _ in 0..2 {
            feed.ingest(&mut source, &mut receiver, Duration::ZERO)
                .await
                .unwrap();
        }
        feed.release_source(&source).await.unwrap();
        let event = feed
            .read(None, 100)
            .await
            .unwrap()
            .events()
            .iter()
            .find(|e| e.has_callback_query())
            .unwrap()
            .clone();
        let actor = Principal::new("host").unwrap();
        let policy = PolicyStore::default();
        let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
        executor.register_profile(feed, client.clone()).unwrap();
        let file = executor
            .register_file(
                &actor,
                "personal",
                "sample.bin",
                b"abc".to_vec(),
                now() + 60_000,
            )
            .await
            .unwrap();
        let peer = PeerId::new(PeerKind::User, 42).unwrap();
        let mut grant = Grant::new(
            ProfileIdentity::mtproto_bot(42).unwrap(),
            Budget::new("daily").unwrap(),
        )
        .unwrap()
        .peers(PeerScope::only([peer]))
        .files([file.handle()]);
        for action in Action::ALL {
            grant = grant.allow(action);
        }
        policy.replace_mtproto(&actor, "personal", grant).unwrap();
        let alias = format!("mtcb_{}", "a".repeat(32));
        let reference = executor
            .observe_callback_as(&actor, "personal", &event, &alias)
            .unwrap();
        let bytes_path = self.dir.path().join("bytes.bin");
        std::fs::write(&bytes_path, b"abc").unwrap();
        self.config["callback_clock_policy"] = json!("trusted_wall_clock");
        self.config["mtproto"]["profiles"][0]["request_timeout_ms"] = json!(500);
        self.config["mtproto"]["profiles"][0]["grant"]["files"] = json!([file.handle()]);
        self.config["mtproto"]["file_restorations"] =
            json!([{"profile":"personal","file_handle":file.handle(),"path":bytes_path}]);
        self.config["mtproto"]["callback_observations"] =
            json!([{"profile":"personal","cursor":event.cursor(),"reference":alias}]);
        let requests = vec![
            request("mtproto_get_me", json!({}), None),
            request("mtproto_get_peer", json!({"peer":peer}), None),
            request("mtproto_read_updates", json!({}), None),
            request(
                "mtproto_send_message",
                json!({"peer":peer,"text":"PRIVATE_TEXT"}),
                Some("send"),
            ),
            request(
                "mtproto_edit_message_text",
                json!({"peer":peer,"message_id":"1","text":"PRIVATE_EDIT"}),
                Some("edit"),
            ),
            request(
                "mtproto_answer_callback_query",
                json!({"reference":alias,"text":"PRIVATE_ANSWER"}),
                Some("callback"),
            ),
            request(
                "mtproto_get_operation",
                json!({"operation_id":"send"}),
                None,
            ),
            request(
                "mtproto_send_document",
                json!({"peer":peer,"file_handle":file.handle()}),
                Some("document"),
            ),
        ];
        let writes = requests
            .iter()
            .filter(|r| r.get("operation_id").is_some())
            .collect::<Vec<_>>();
        self.config["mtproto"]["profiles"][0]["grant"]["require_approval"] = json!(
            writes
                .iter()
                .map(|r| r["call"]["tool"].clone())
                .collect::<Vec<_>>()
        );
        self.config["mtproto"]["approvals"]=json!(writes.iter().map(|r|json!({"request":r,"expires_at_ms":(now()+5000).min(reference.expires_at_ms()-100)})).collect::<Vec<_>>());
        executor.close().await.unwrap();
        client.close().await.unwrap();
        journal.close().await.unwrap();
        requests
    }
    async fn synthetic(&self, mode: &str, input: &[u8], pending: bool) -> std::process::Output {
        self.synthetic_config(&self.config, mode, input, pending)
            .await
    }
    async fn synthetic_config(
        &self,
        configuration: &Value,
        mode: &str,
        input: &[u8],
        pending: bool,
    ) -> std::process::Output {
        let config = self.dir.path().join("host.json");
        std::fs::write(&config, serde_json::to_vec(configuration).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tg-agent-mtproto-fixture"));
        command
            .arg(mode)
            .arg(config)
            .env(KEY_ENV, "07".repeat(32))
            .env("TG_MT_FIXTURE_COUNTS", self.dir.path().join("counts.json"))
            .env(
                "TG_MT_FIXTURE_REOPEN",
                self.dir.path().join("session.sqlite"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if pending {
            command.env("TG_MT_FIXTURE_MODE", "timeout");
        }
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).await.unwrap();
        drop(stdin);
        tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn validate(value: &Value, request: &Value) {
    let tool = tg_agent::tools::extended_catalog()
        .tools
        .into_iter()
        .find(|t| t.name == request["call"]["tool"].as_str().unwrap())
        .unwrap();
    let schema = jsonschema::validator_for(&tool.output_schema).unwrap();
    let errors = schema
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:?}: {value}");
    assert_eq!(value["ok"], true, "{value}");
    assert!(!value.to_string().contains("PRIVATE"));
}
fn confirmed(value: &Value) {
    let Some(operation) = value["result"].get("operation") else {
        return;
    };
    assert_eq!(operation["state"], "succeeded");
    assert_eq!(operation["result_availability"], "stored");
    if operation["action"] == "mtproto_answer_callback_query" {
        assert_eq!(operation["acknowledged"], true);
        assert!(operation.get("message").is_none());
    } else {
        assert_eq!(
            operation["message"],
            json!({"peer":{"kind":"user","id":"42"},"message_id":"1"})
        );
    }
}
fn decode(out: &std::process::Output) -> Value {
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE"));
    value
}
#[tokio::test]
async fn ordinary_cli_loads_mt_profile_without_rpc_dry_runs_and_releases_the_session() {
    let f = Fixture::new().await;
    let input = request(
        "mtproto_send_message",
        json!({"peer":{"kind":"user","id":"42"},"text":"PRIVATE_TEXT"}),
        Some("dry"),
    );
    let out = f
        .run(&f.config, "run", &input, true, Some(&"07".repeat(32)))
        .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let value = decode(&out);
    assert_eq!(value["dry_run"]["action"], "mtproto_send_message");
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    let out = f
        .run(
            &f.config,
            "run",
            &request("mtproto_read_updates", json!({}), None),
            false,
            Some(&"07".repeat(32)),
        )
        .await;
    assert!(out.status.success());
    let value = decode(&out);
    assert!(
        value["result"]["updates"]["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    f.no_connections().await;
}
#[tokio::test]
async fn static_mt_config_errors_precede_journal_creation_and_never_echo_credentials() {
    let f = Fixture::new().await;
    let mutations = [
        ("/schema_version", json!(1)),
        ("/mtproto", Value::Null),
        ("/mtproto/profiles/0/account_id", json!("042")),
        ("/mtproto/profiles/0/account_id", json!("0")),
        ("/mtproto/profiles/0/account_kind", json!("user")),
        (
            "/mtproto/profiles/0/session_path",
            json!("PRIVATE_RELATIVE"),
        ),
        ("/mtproto/profiles/0/session_key_env", json!("PRIVATE=BAD")),
        ("/mtproto/profiles/0/request_timeout_ms", json!(30001)),
        ("/mtproto/profiles/0/max_pending", json!(0)),
        ("/mtproto/profiles/0/grant/actions", json!(["get_me"])),
        (
            "/mtproto/profiles/0/grant/peers",
            json!([{"kind":"user","id":"0"}]),
        ),
    ];
    for (pointer, value) in mutations {
        let mut config = f.config.clone();
        *config
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing {pointer}")) = value;
        let out = f
            .run(
                &config,
                "run",
                &request("mtproto_read_updates", json!({}), None),
                false,
                Some(&"07".repeat(32)),
            )
            .await;
        assert_eq!(out.status.code(), Some(2), "{pointer}");
        assert_eq!(decode(&out)["error"]["code"], "agent_config");
        assert!(!f.dir.path().join("agent.sqlite").exists(), "{pointer}");
    }
}
#[tokio::test]
async fn missing_wrong_key_binding_and_account_refuse_before_journal_and_close_opened_stores() {
    let f = Fixture::new().await;
    let input = request("mtproto_read_updates", json!({}), None);
    for key in [None, Some("PRIVATE_BAD".to_owned()), Some("08".repeat(32))] {
        let out = f.run(&f.config, "run", &input, false, key.as_deref()).await;
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(decode(&out)["error"]["code"], "agent_config");
        assert!(!f.dir.path().join("agent.sqlite").exists());
    }
    for (field, value) in [
        ("session_binding", json!("foreign")),
        ("account_id", json!("43")),
        ("session_path", json!(f.dir.path().join("missing.sqlite"))),
    ] {
        let mut config = f.config.clone();
        config["mtproto"]["profiles"][0][field] = value;
        let out = f
            .run(&config, "run", &input, false, Some(&"07".repeat(32)))
            .await;
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(decode(&out)["error"]["code"], "agent_config");
        assert!(!f.dir.path().join("agent.sqlite").exists());
        f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    }
    assert!(!f.dir.path().join("missing.sqlite").exists());
}

#[tokio::test]
async fn undeclared_callback_alias_approval_is_rejected_before_any_storage_is_opened() {
    let mut f = Fixture::new().await;
    let input = request(
        "mtproto_answer_callback_query",
        json!({"reference":format!("mtcb_{}","a".repeat(32))}),
        Some("unobserved"),
    );
    f.config["mtproto"]["approvals"] = json!([{"request":input,"expires_at_ms":now()+1000}]);
    let out = f
        .run(&f.config, "run", &input, false, Some(&"07".repeat(32)))
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    assert!(
        !f.dir.path().join("agent.sqlite").exists(),
        "undeclared alias reached journal creation"
    );
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
}

#[tokio::test]
async fn mixed_protocol_config_keeps_bot_requests_and_global_profile_names_exact() {
    let mut f = Fixture::new().await;
    f.config["profiles"] = json!([{"name":"demo","bot_id":"0","token_env":"TG_MT_HOST_BOT_TOKEN","grant":{"actions":["send_message"],"chats":["1"],"budget":{"id":"bot-daily"}}}]);
    let input = json!({"schema_version":1,"profile":"demo","operation_id":"bot-dry","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"PRIVATE_BOT"}}});
    let out = f
        .run(&f.config, "run", &input, true, Some(&"07".repeat(32)))
        .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(decode(&out)["dry_run"]["action"], "send_message");
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    let fresh = f.dir.path().join("must-not-exist.sqlite");
    f.config["journal_path"] = json!(fresh);
    f.config["mtproto"]["profiles"][0]["name"] = json!("demo");
    let out = f
        .run(&f.config, "run", &input, true, Some(&"07".repeat(32)))
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    assert!(!fresh.exists());
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn ordinary_mcp_executable_advertises_the_mt_host_catalog_without_any_rpc() {
    use tokio::io::AsyncBufReadExt;
    let f = Fixture::new().await;
    let config = f.dir.path().join("host.json");
    std::fs::write(&config, serde_json::to_vec(&f.config).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tg-agent"))
        .arg("mcp")
        .arg("--config")
        .arg(config)
        .env(KEY_ENV, "07".repeat(32))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"offline","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let mut frame = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":meta}}),
    )
    .unwrap();
    frame.push(b'\n');
    stdin.write_all(&frame).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 16);
    drop(stdin);
    drop(stdout);
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    f.no_connections().await;
}

#[tokio::test]
async fn hard_link_journal_cannot_modify_or_adopt_the_session_database() {
    let f = Fixture::new().await;
    let session = f.dir.path().join("session.sqlite");
    let alias = f.dir.path().join("hard-link.sqlite");
    std::fs::hard_link(&session, &alias).unwrap();
    let original = std::fs::read(&session).unwrap();
    let mut config = f.config.clone();
    config["journal_path"] = json!(alias);
    let out = f
        .run(
            &config,
            "run",
            &request("mtproto_read_updates", json!({}), None),
            false,
            Some(&"07".repeat(32)),
        )
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    assert!(
        std::fs::read(&session).unwrap() == original,
        "a hard-link journal modified the encrypted session database"
    );
    f.reusable(42, &session).await;
}

#[tokio::test]
async fn host_bootstrap_executes_all_eight_cli_tools_with_restored_files_callbacks_and_approvals() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    let mut input = Vec::new();
    for request in &requests {
        input.extend_from_slice(&serde_json::to_vec(request).unwrap());
        input.push(b'\n');
    }
    for request in requests.iter().filter(|r| r.get("operation_id").is_some()) {
        input.extend_from_slice(&serde_json::to_vec(request).unwrap());
        input.push(b'\n');
    }
    let out = f.synthetic("serve", &input, false).await;
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let frames = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 12);
    for (value, request) in frames.iter().zip(
        requests
            .iter()
            .chain(requests.iter().filter(|r| r.get("operation_id").is_some())),
    ) {
        validate(value, request);
        confirmed(value);
    }
    assert_eq!(
        frames[2]["result"]["updates"]["events"][0]["callback_reference"],
        f.config["mtproto"]["callback_observations"][0]["reference"]
    );
    let counts: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("counts.json")).unwrap()).unwrap();
    assert_eq!(counts["writes"], 4);
    for (cached, original) in frames[8..].iter().zip([3usize, 4, 5, 7]) {
        assert_eq!(cached, &frames[original]);
    }
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    let lookup = request(
        "mtproto_get_operation",
        json!({"operation_id":"send"}),
        None,
    );
    let out = f
        .run(&f.config, "run", &lookup, false, Some(&"07".repeat(32)))
        .await;
    assert!(out.status.success());
    validate(&decode(&out), &lookup);
    confirmed(&decode(&out));
    let out = f
        .run(
            &f.config,
            "run",
            &requests[2],
            false,
            Some(&"07".repeat(32)),
        )
        .await;
    assert!(out.status.success());
    let page = decode(&out);
    validate(&page, &requests[2]);
    assert!(
        !page["result"]["updates"]["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.no_connections().await;
}

#[tokio::test]
async fn standalone_unknown_survives_restart_and_a_same_key_never_dispatches_again() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    let mut input = serde_json::to_vec(&requests[3]).unwrap();
    input.push(b'\n');
    let out = f.synthetic("serve", &input, true).await;
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    validate(&value, &requests[3]);
    assert_eq!(value["result"]["operation"]["state"], "unknown");
    let out = f.synthetic("serve", &input, false).await;
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    validate(&value, &requests[3]);
    assert_eq!(value["result"]["operation"]["state"], "unknown");
    let counts: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("counts.json")).unwrap()).unwrap();
    assert_eq!(
        counts,
        json!({"calls":0,"writes":0,"reopened_before_exit":true})
    );
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
}

#[tokio::test]
async fn host_loaded_exact_approval_rejects_changed_payload_or_key_before_sdk_dispatch() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    for change_key in [false, true] {
        let mut changed = requests[3].clone();
        if change_key {
            changed["operation_id"] = json!("changed-key");
        } else {
            changed["call"]["arguments"]["text"] = json!("changed payload");
        }
        let mut frame = serde_json::to_vec(&changed).unwrap();
        frame.push(b'\n');
        let out = f.synthetic("serve", &frame, false).await;
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "executor_approval_required");
        let schema = tg_agent::tools::extended_catalog()
            .tools
            .into_iter()
            .find(|v| v.name == "mtproto_send_message")
            .unwrap()
            .output_schema;
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        let counts: Value =
            serde_json::from_slice(&std::fs::read(f.dir.path().join("counts.json")).unwrap())
                .unwrap();
        assert_eq!(counts["calls"], 0);
        assert_eq!(counts["writes"], 0);
        assert_eq!(counts["reopened_before_exit"], true);
    }
}

#[tokio::test]
async fn host_restoration_hash_and_callback_scope_clock_and_expiry_refuse_without_rpc() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    let bytes = std::path::Path::new(
        f.config["mtproto"]["file_restorations"][0]["path"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(bytes, b"abd").unwrap();
    let out = f
        .run(
            &f.config,
            "run",
            &requests[2],
            false,
            Some(&"07".repeat(32)),
        )
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    let proof = f.synthetic("startup-failure", b"", false).await;
    assert!(proof.status.success());
    assert_eq!(decode(&proof)["reopened_before_exit"], true);
    std::fs::write(bytes, b"abc").unwrap();
    for scope in [false, true] {
        let mut config = f.config.clone();
        config["mtproto"]["approvals"] = json!([]);
        if scope {
            config["mtproto"]["profiles"][0]["grant"]["peers"] = json!([{"kind":"user","id":"43"}]);
        } else {
            config["callback_clock_policy"] = json!("journal_monotonic");
        }
        let out = f
            .run(&config, "run", &requests[2], false, Some(&"07".repeat(32)))
            .await;
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(decode(&out)["error"]["code"], "agent_config");
        f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    }
    let connection = rusqlite::Connection::open(f.dir.path().join("agent.sqlite")).unwrap();
    connection.execute("UPDATE mtproto_updates SET received_at_ms=?1 WHERE profile='personal' AND callback_query IS NOT NULL",[now()-11000]).unwrap();
    drop(connection);
    let out = f
        .run(
            &f.config,
            "run",
            &requests[2],
            false,
            Some(&"07".repeat(32)),
        )
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    f.no_connections().await;
}

#[tokio::test]
async fn partial_session_and_post_journal_startup_failures_join_every_accepted_client() {
    let f = Fixture::new().await;
    let other = f.dir.path().join("other.sqlite");
    provision(&other, 43).await;
    let mut config = f.config.clone();
    let mut second = config["mtproto"]["profiles"][0].clone();
    second["name"] = json!("second");
    second["account_id"] = json!("43");
    second["session_path"] = json!(other);
    second["session_binding"] = json!("foreign");
    config["mtproto"]["profiles"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let input = request("mtproto_read_updates", json!({}), None);
    let out = f
        .run(&config, "run", &input, false, Some(&"07".repeat(32)))
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    assert!(!f.dir.path().join("agent.sqlite").exists());
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    f.reusable(43, &other).await;
    let proof = f
        .synthetic_config(&config, "startup-failure", b"", false)
        .await;
    assert!(
        proof.status.success(),
        "{}",
        String::from_utf8_lossy(&proof.stderr)
    );
    assert_eq!(decode(&proof)["reopened_before_exit"], true);
    assert_eq!(decode(&proof)["accepted_clients"], 1);
    let mut config = f.config.clone();
    let path = f.dir.path().join("input.bin");
    std::fs::write(&path, b"abc").unwrap();
    config["mtproto"]["file_restorations"] =
        json!([{"profile":"personal","file_handle":"unknown","path":path}]);
    let out = f
        .run(&config, "run", &input, false, Some(&"07".repeat(32)))
        .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(decode(&out)["error"]["code"], "agent_config");
    assert!(f.dir.path().join("agent.sqlite").exists());
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    let proof = f
        .synthetic_config(&config, "startup-failure", b"", false)
        .await;
    assert!(
        proof.status.success(),
        "{}",
        String::from_utf8_lossy(&proof.stderr)
    );
    assert_eq!(decode(&proof)["reopened_before_exit"], true);
}

#[tokio::test]
async fn cancelled_host_close_wait_can_resume_and_preserves_the_owned_confirmation() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    f.config["mtproto"]["profiles"][0]["request_timeout_ms"] = json!(10000);
    let out = f
        .synthetic(
            "lifecycle",
            &serde_json::to_vec(&requests[3]).unwrap(),
            false,
        )
        .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["close_was_pending"], true);
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
    let journal = tg_agent::journal::Journal::open(
        f.dir.path().join("agent.sqlite"),
        tg_agent::journal::JournalOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        journal
            .get("personal", "host", "send")
            .await
            .unwrap()
            .unwrap()
            .state(),
        tg_agent::journal::OperationState::Succeeded
    );
    journal.close().await.unwrap();
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn standalone_mcp_uses_host_profiles_for_all_eight_actions_and_extended_discovery() {
    let mut f = Fixture::new().await;
    let requests = f.seed().await;
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"offline","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let mut input = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":{"_meta":meta}}),
    )
    .unwrap();
    input.push(b'\n');
    // Send only one outstanding write at a time: CLI covers the serialized batch;
    // raw MCP frames here are tested through a live process below.
    let config = f.dir.path().join("host.json");
    std::fs::write(&config, serde_json::to_vec(&f.config).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tg-agent-mtproto-fixture"))
        .arg("mcp")
        .arg(config)
        .env(KEY_ENV, "07".repeat(32))
        .env("TG_MT_FIXTURE_COUNTS", f.dir.path().join("counts.json"))
        .env("TG_MT_FIXTURE_REOPEN", f.dir.path().join("session.sqlite"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    use tokio::io::AsyncBufReadExt;
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
    stdin.write_all(&input).await.unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).await.unwrap();
    let discovery: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(discovery["result"]["tools"].as_array().unwrap().len(), 16);
    for (index, request) in requests.iter().enumerate() {
        let mut frame=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":index+1,"method":"tools/call","params":{"name":request["call"]["tool"],"arguments":request,"_meta":meta}})).unwrap();
        frame.push(b'\n');
        stdin.write_all(&frame).await.unwrap();
        line.clear();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], index + 1);
        validate(&response["result"]["structuredContent"], request);
        confirmed(&response["result"]["structuredContent"]);
    }
    drop(stdin);
    drop(stdout);
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let counts: Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("counts.json")).unwrap()).unwrap();
    assert_eq!(counts["writes"], 4);
    f.reusable(42, &f.dir.path().join("session.sqlite")).await;
}
