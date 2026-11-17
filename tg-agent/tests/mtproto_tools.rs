#![cfg(feature = "mtproto-test-kit")]
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tg_agent::{
    executor::{Executor as BotExecutor, ExecutorOptions as BotOptions},
    journal::{Journal, JournalOptions},
    mtproto::{Action, Executor, ExecutorOptions, PolicyStore, Principal, Request},
    mtproto::{
        Budget, Grant, PeerId, PeerKind, PeerScope,
        updates::{Feed, Options},
    },
    profiles::ProfileIdentity,
    tools::{ExecutionMode, ToolRequest, ToolSession, catalog, extended_catalog},
};
use tg_mtproto::{
    AccountId, AccountIdentity, AccountKind, Client, ClientOptions, Session, SessionBinding,
    SessionKey, SessionOptions, UpdateMode, test_kit, tl,
};
use tl::{Deserializable, Identifiable, Serializable};

#[test]
fn extended_discovery_preserves_bot_inventory_and_validates_typed_examples() {
    let old = serde_json::to_value(catalog()).unwrap();
    let extended = extended_catalog();
    assert_eq!(extended.schema_version, 2);
    assert_eq!(extended.tools.len(), 16);
    let value = serde_json::to_value(&extended).unwrap();
    assert_eq!(
        value["tools"].as_array().unwrap()[..8],
        old["tools"].as_array().unwrap()[..]
    );
    for (tool, action) in extended.tools[8..].iter().zip(Action::ALL) {
        assert_eq!(tool.name, action.name());
        let input = jsonschema::validator_for(&tool.input_schema).unwrap();
        let output = jsonschema::validator_for(&tool.output_schema).unwrap();
        let example = &tool.examples[0];
        assert!(input.is_valid(&example["input"]), "{} input", tool.name);
        assert!(output.is_valid(&example["success"]), "{} output", tool.name);
        assert!(output.is_valid(&example["error"]), "{} error", tool.name);
        let bytes = serde_json::to_vec(&example["input"]).unwrap();
        assert_eq!(Request::from_json(&bytes).unwrap().action(), action);
        assert_eq!(ToolRequest::from_json(&bytes).unwrap().name(), tool.name);
    }
}

#[tokio::test]
async fn provider_attachment_selected_name_and_strict_parsing_do_not_grant_authority() {
    let policy = PolicyStore::default();
    let session = ToolSession::new(
        BotExecutor::new(policy.clone(), BotOptions::default()).unwrap(),
        Principal::new("host").unwrap(),
    );
    assert_eq!(session.catalog().tools.len(), 8);
    let example = &extended_catalog().tools[11].examples[0]["input"];
    let bytes = serde_json::to_vec(example).unwrap();
    let request = ToolRequest::from_json(&bytes).unwrap();
    let result = session.dispatch_json(&bytes, ExecutionMode::Execute).await;
    let error = serde_json::to_value(result).unwrap();
    assert_eq!(error["error"]["code"], "agent_protocol_unavailable");
    assert_eq!(error["error"]["next_action"], "mtproto_get_operation");
    let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
    let session = session.with_mtproto(executor.clone());
    assert_eq!(session.catalog().tools.len(), 16);
    let result = session
        .execute_named("send_message", &request, ExecutionMode::Execute)
        .await;
    assert_eq!(
        serde_json::to_value(result).unwrap()["error"]["code"],
        "agent_tool_mismatch"
    );
    for raw in [
        r#"{"schema_version":1,"profile":"demo","call":{"tool":"mtproto_get_me","arguments":{}},"principal":"PRIVATE"}"#,
        r#"{"schema_version":1,"profile":"demo","call":{"tool":"mtproto_get_me","arguments":[]}}"#,
        r#"{"schema_version":1,"profile":"demo","profile":"other","call":{"tool":"mtproto_get_me","arguments":{}}}"#,
        r#"{"schema_version":1,"profile":"demo","call":{"tool":"mtproto_get_me","arguments":{},"arguments":{}}}"#,
    ] {
        assert!(ToolRequest::from_json(raw.as_bytes()).is_err());
        let error = session
            .dispatch_json(raw.as_bytes(), ExecutionMode::Execute)
            .await;
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["error"]["code"], "executor_invalid_input");
        assert!(!json.to_string().contains("PRIVATE"));
    }
    let unconfigured = session.dispatch_json(&bytes, ExecutionMode::Execute).await;
    assert!(unconfigured.is_error());
    executor.close().await.unwrap();
}

#[test]
fn tagged_peer_and_mt_target_schemas_match_runtime_boundaries() {
    let extended = extended_catalog();
    let definition = &extended.tools[12];
    let validator = jsonschema::validator_for(&definition.input_schema).unwrap();
    for kind in ["user", "chat", "channel"] {
        for id in [
            "1",
            "0",
            "-1",
            "01",
            "9223372036854775807",
            "999999999999",
            "1000000000000",
            "997852516352",
            "997852516353",
            "1002147483648",
            "1002147483649",
            "3000000000000",
            "3000000000001",
            "1099511627775",
            "1099511627776",
        ] {
            for message in ["1", "2147483647", "2147483648"] {
                let mut value = definition.examples[0]["input"].clone();
                value["call"]["arguments"]["peer"] = json!({"kind":kind,"id":id});
                value["call"]["arguments"]["message_id"] = json!(message);
                let runtime = Request::from_json(&serde_json::to_vec(&value).unwrap()).is_ok();
                assert_eq!(validator.is_valid(&value), runtime, "{kind}/{id}/{message}");
            }
        }
    }
    let mut injected: Value = definition.examples[0]["input"].clone();
    injected["call"]["arguments"]["peer"]["access_hash"] = json!("PRIVATE");
    assert!(!validator.is_valid(&injected));
    assert!(ToolRequest::from_json(&serde_json::to_vec(&injected).unwrap()).is_err());
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn peer() -> PeerId {
    PeerId::new(PeerKind::User, 42).unwrap()
}
fn request(tool: &str, args: Value, key: Option<&str>) -> Request {
    let mut v =
        json!({"schema_version":1,"profile":"personal","call":{"tool":tool,"arguments":args}});
    if let Some(key) = key {
        v["operation_id"] = json!(key);
    }
    Request::from_json(&serde_json::to_vec(&v).unwrap()).unwrap()
}
fn send(key: &str) -> Request {
    request(
        "mtproto_send_message",
        json!({"peer":peer(),"text":"PRIVATE_TEXT"}),
        Some(key),
    )
}
fn sent(media: Option<tl::enums::MessageMedia>) -> Vec<u8> {
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id: 1,
        pts: 1,
        pts_count: 1,
        date: 123,
        media,
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}
fn media() -> tl::enums::MessageMedia {
    let mut v = tl::types::MessageMediaDocument::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&tl::types::Document::CONSTRUCTOR_ID.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&1i64.to_le_bytes());
    v.extend_from_slice(&777i64.to_le_bytes());
    v.extend_from_slice(&b"PRIVATE_FILE_REFERENCE".to_vec().to_bytes());
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&"application/octet-stream".to_owned().to_bytes());
    v.extend_from_slice(&3i64.to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(
        &vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "sample.bin".into(),
            },
        )]
        .to_bytes(),
    );
    tl::enums::MessageMedia::from_bytes(&v).unwrap()
}
fn self_user() -> tl::enums::User {
    let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&((1u32 << 10) | (1 << 14)).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&42i64.to_le_bytes());
    v.extend_from_slice(&0i32.to_le_bytes());
    tl::enums::User::from_bytes(&v).unwrap()
}
fn edited() -> Vec<u8> {
    let mut v = tl::types::Message::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&2u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&1i32.to_le_bytes());
    v.extend_from_slice(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 42 }).to_bytes());
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&"PRIVATE_EDIT".to_owned().to_bytes());
    tl::enums::Updates::Updates(tl::types::Updates {
        updates: vec![
            tl::types::UpdateEditMessage {
                message: tl::enums::Message::from_bytes(&v).unwrap(),
                pts: 2,
                pts_count: 1,
            }
            .into(),
        ],
        users: vec![],
        chats: vec![],
        date: 123,
        seq: 2,
    })
    .to_bytes()
}
fn state(pts: i32) -> tl::enums::updates::State {
    tl::types::updates::State {
        pts,
        qts: 0,
        seq: 0,
        date: 100 + pts,
        unread_count: 0,
    }
    .into()
}
#[derive(Default)]
struct Control {
    calls: AtomicUsize,
    writes: AtomicUsize,
    pending: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
struct Fixture {
    _dir: tempfile::TempDir,
    journal: Journal,
    client: Client,
    executor: Executor,
    policy: PolicyStore,
    session: ToolSession,
    handle: String,
    reference: String,
    control: Arc<Control>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(dir.path().join("agent.sqlite"), JournalOptions::default())
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
        let stored = Session::open(
            dir.path().join("session.sqlite"),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .unwrap();
        let control = Arc::new(Control::default());
        let c = control.clone();
        let client = test_kit::client(
            stored,
            ClientOptions::default().request_timeout(Duration::from_secs(2)),
            move |_, bytes| {
                let c = c.clone();
                async move {
                    c.calls.fetch_add(1, Ordering::SeqCst);
                    let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                    if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
                        return Ok(vec![self_user()].to_bytes());
                    }
                    if ctor == tl::functions::updates::GetState::CONSTRUCTOR_ID {
                        return Ok(state(1).to_bytes());
                    }
                    if ctor == tl::functions::updates::GetDifference::CONSTRUCTOR_ID {
                        return Ok(tl::enums::updates::Difference::Difference(
                            tl::types::updates::Difference {
                                new_messages: vec![],
                                new_encrypted_messages: vec![],
                                chats: vec![],
                                users: vec![],
                                other_updates: vec![
                                    tl::types::UpdateBotCallbackQuery {
                                        query_id: 3,
                                        user_id: 99,
                                        peer: tl::types::PeerUser { user_id: 42 }.into(),
                                        msg_id: 1,
                                        chat_instance: 888,
                                        data: Some(vec![b'x'; 2048]),
                                        game_short_name: None,
                                    }
                                    .into(),
                                ],
                                state: state(2),
                            },
                        )
                        .to_bytes());
                    }
                    if ctor == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID {
                        let part =
                            tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
                        assert_eq!(part.bytes, b"abc");
                        return Ok(true.to_bytes());
                    }
                    c.writes.fetch_add(1, Ordering::SeqCst);
                    c.entered.notify_one();
                    if c.pending.load(Ordering::SeqCst) {
                        c.release.notified().await;
                    }
                    if ctor == tl::functions::messages::SendMessage::CONSTRUCTOR_ID {
                        return Ok(sent(None));
                    }
                    if ctor == tl::functions::messages::EditMessage::CONSTRUCTOR_ID {
                        return Ok(edited());
                    }
                    if ctor == tl::functions::messages::SendMedia::CONSTRUCTOR_ID {
                        return Ok(sent(Some(media())));
                    }
                    assert_eq!(
                        ctor,
                        tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID
                    );
                    assert_eq!(
                        tl::functions::messages::SetBotCallbackAnswer::from_bytes(&bytes[4..])
                            .unwrap()
                            .query_id,
                        3
                    );
                    Ok(true.to_bytes())
                }
            },
        )
        .unwrap();
        let expected = AccountIdentity {
            id: AccountId::new(42).unwrap(),
            kind: AccountKind::Bot,
        };
        let mut source = feed.claim_source().await.unwrap();
        let mut receiver = client
            .account(expected)
            .updates(UpdateMode::StartNow)
            .unwrap();
        for _ in 0..2 {
            feed.ingest(&mut source, &mut receiver, Duration::ZERO)
                .await
                .unwrap();
        }
        feed.release_source(&source).await.unwrap();
        let policy = PolicyStore::default();
        let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
        executor.register_profile(feed, client.clone()).unwrap();
        let actor = Principal::new("host").unwrap();
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
        policy
            .replace_mtproto(&actor, "personal", Self::grant(file.handle()))
            .unwrap();
        let reference = executor
            .execute(
                &actor,
                &request("mtproto_read_updates", json!({}), None),
                None,
            )
            .await
            .unwrap()
            .read_page()
            .unwrap()
            .events()
            .iter()
            .find_map(|e| e.callback_reference().map(str::to_owned))
            .unwrap();
        let session = ToolSession::new(
            BotExecutor::new(policy.clone(), BotOptions::default()).unwrap(),
            actor,
        )
        .with_mtproto(executor.clone());
        Self {
            _dir: dir,
            journal,
            client,
            executor,
            policy,
            session,
            handle: file.handle().into(),
            reference,
            control,
        }
    }
    fn grant(handle: &str) -> Grant {
        let mut grant = Grant::new(
            ProfileIdentity::mtproto_bot(42).unwrap(),
            Budget::new("daily").unwrap(),
        )
        .unwrap()
        .peers(PeerScope::only([peer()]))
        .files([handle]);
        for action in Action::ALL {
            grant = grant.allow(action);
        }
        grant
    }
    fn requests(&self) -> Vec<Request> {
        vec![
            request("mtproto_get_me", json!({}), None),
            request("mtproto_get_peer", json!({"peer":peer()}), None),
            request("mtproto_read_updates", json!({}), None),
            send("message"),
            request(
                "mtproto_edit_message_text",
                json!({"peer":peer(),"message_id":"1","text":"PRIVATE_EDIT"}),
                Some("edit"),
            ),
            request(
                "mtproto_answer_callback_query",
                json!({"reference":self.reference,"text":"PRIVATE_ANSWER"}),
                Some("callback"),
            ),
            request(
                "mtproto_get_operation",
                json!({"operation_id":"message"}),
                None,
            ),
            request(
                "mtproto_send_document",
                json!({"peer":peer(),"file_handle":self.handle}),
                Some("document"),
            ),
        ]
    }
    async fn close(self) {
        self.executor.close().await.unwrap();
        self.client.close().await.unwrap();
        self.journal.close().await.unwrap();
    }
}
fn validate(request: &Request, response: &Value) {
    validate_shape(request, response);
    assert_eq!(response["ok"], true, "{response}");
}
fn confirmed(response: &Value) {
    let Some(operation) = response["result"].get("operation") else {
        return;
    };
    assert_eq!(operation["state"], "succeeded");
    assert_eq!(operation["result_availability"], "stored");
    if operation["action"] == "mtproto_answer_callback_query" {
        assert_eq!(operation["acknowledged"], true);
    } else {
        assert_eq!(
            operation["message"],
            json!({"peer":{"kind":"user","id":"42"},"message_id":"1"})
        );
    }
}
fn validate_shape(request: &Request, response: &Value) {
    let definition = extended_catalog()
        .tools
        .into_iter()
        .find(|tool| tool.name == request.action().name())
        .unwrap();
    let validator = jsonschema::validator_for(&definition.output_schema).unwrap();
    let errors = validator
        .iter_errors(response)
        .map(|e| e.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "{}: {errors:?}: {response}",
        request.action().name()
    );
    assert!(!response.to_string().contains("PRIVATE"));
}

#[cfg(feature = "cli")]
#[tokio::test]
async fn all_eight_actions_use_embedded_cli_and_exact_cached_receipts() {
    use tg_agent::tools::OutputLimit;
    let f = Fixture::new().await;
    let requests = f.requests();
    for request in &requests {
        let input = serde_json::to_vec(request).unwrap();
        let mut output = Vec::new();
        assert!(
            !tg_agent::cli::run_json(
                &f.session,
                input.as_slice(),
                &mut output,
                ExecutionMode::Execute,
                OutputLimit::default()
            )
            .await
            .unwrap()
        );
        let response = serde_json::from_slice::<Value>(&output).unwrap();
        validate(request, &response);
        confirmed(&response);
    }
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 4);
    f.executor
        .discard_file(&Principal::new("host").unwrap(), "personal", &f.handle)
        .unwrap();
    let calls = f.control.calls.load(Ordering::SeqCst);
    for request in requests.iter().filter(|r| r.action().is_write()) {
        let output = f
            .session
            .dispatch_json(
                &serde_json::to_vec(request).unwrap(),
                ExecutionMode::Execute,
            )
            .await;
        let response = serde_json::to_value(output).unwrap();
        validate(request, &response);
        confirmed(&response);
    }
    for request in &requests {
        let dry = f
            .session
            .dispatch_json(&serde_json::to_vec(request).unwrap(), ExecutionMode::DryRun)
            .await;
        let dry = serde_json::to_value(dry).unwrap();
        validate(request, &dry);
        assert_eq!(dry["dry_run"]["action"], request.action().name());
    }
    assert_eq!(f.control.calls.load(Ordering::SeqCst), calls);
    f.close().await;
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn all_eight_actions_use_official_mcp_and_the_session_catalog() {
    use rmcp::{ServiceExt, model::CallToolRequestParams};
    use tg_agent::{mcp::McpServer, tools::OutputLimit};
    let f = Fixture::new().await;
    let (client_io, server_io) = tokio::io::duplex(512 * 1024);
    let server = tokio::spawn(
        McpServer::new(
            f.session.clone(),
            ExecutionMode::Execute,
            OutputLimit::default(),
        )
        .serve(server_io),
    );
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 16);
    for (actual, expected) in tools.iter().zip(extended_catalog().tools) {
        assert_eq!(actual.name, expected.name);
        assert_eq!(
            Value::Object((*actual.input_schema).clone()),
            expected.input_schema
        );
        assert_eq!(
            Value::Object((**actual.output_schema.as_ref().unwrap()).clone()),
            expected.output_schema
        );
    }
    let before = f.control.calls.load(Ordering::SeqCst);
    let wrong = client
        .call_tool(
            CallToolRequestParams::new("send_message").with_arguments(
                serde_json::to_value(send("wrong-selected"))
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let wrong = wrong.structured_content.unwrap();
    assert_eq!(wrong["error"]["code"], "agent_tool_mismatch");
    assert_eq!(wrong["error"]["next_action"], "mtproto_get_operation");
    validate_shape(&send("wrong-selected"), &wrong);
    assert_eq!(f.control.calls.load(Ordering::SeqCst), before);
    for request in f.requests() {
        let args = serde_json::to_value(&request)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        let output = client
            .call_tool(CallToolRequestParams::new(request.action().name()).with_arguments(args))
            .await
            .unwrap();
        assert_eq!(output.is_error, Some(false));
        validate(&request, output.structured_content.as_ref().unwrap());
        confirmed(output.structured_content.as_ref().unwrap());
    }
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 4);
    client.cancel().await.unwrap();
    server.await.unwrap().unwrap();
    f.close().await;
}

#[tokio::test]
async fn exact_mt_approval_dry_run_and_revocation_share_current_policy() {
    let f = Fixture::new().await;
    let actor = Principal::new("host").unwrap();
    let grant = Fixture::grant(&f.handle).require_approval(Action::SendMessage);
    f.policy
        .replace_mtproto(&actor, "personal", grant.clone())
        .unwrap();
    let approved = send("approved");
    let input = serde_json::to_vec(&approved).unwrap();
    let before = f.control.calls.load(Ordering::SeqCst);
    assert!(
        f.session
            .dispatch_json(&input, ExecutionMode::DryRun)
            .await
            .is_error()
    );
    f.session
        .approve_mtproto(&approved, now() + 30_000)
        .await
        .unwrap();
    let dry =
        serde_json::to_value(f.session.dispatch_json(&input, ExecutionMode::DryRun).await).unwrap();
    validate(&approved, &dry);
    assert_eq!(dry["dry_run"]["action"], "mtproto_send_message");
    assert_eq!(f.control.calls.load(Ordering::SeqCst), before);
    assert!(
        f.session
            .dispatch_json(
                &serde_json::to_vec(&send("changed-key")).unwrap(),
                ExecutionMode::Execute
            )
            .await
            .is_error()
    );
    validate(
        &approved,
        &serde_json::to_value(
            f.session
                .dispatch_json(&input, ExecutionMode::Execute)
                .await,
        )
        .unwrap(),
    );
    f.policy.replace_mtproto(&actor, "personal", grant).unwrap();
    assert!(
        f.session
            .dispatch_json(&input, ExecutionMode::Execute)
            .await
            .is_error()
    );
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[test]
fn all_projected_event_variants_validate_after_real_serde_serialization() {
    use tg_agent::mtproto::updates::Event;
    let definition = &extended_catalog().tools[10];
    let schema = &definition.output_schema["oneOf"][0]["properties"]["result"]["properties"]["updates"]
        ["properties"]["events"]["items"]["properties"]["event"];
    let validator = jsonschema::validator_for(schema).unwrap();
    let p = json!(peer());
    for original in [
        json!({"kind":"message","peer":p,"message_id":"1","sender":p,"date":123,"text":"untrusted","outgoing":true,"edited":false,"has_media":true}),
        json!({"kind":"service","peer":p,"message_id":"1","sender":null,"date":i32::MIN,"action":u32::MAX}),
        json!({"kind":"empty","peer":null,"message_id":"1"}),
        json!({"kind":"deleted","peer":p,"message_ids":["1","2147483647"]}),
        json!({"kind":"read_position","peer":p,"max_id":"0","outgoing":false}),
        json!({"kind":"callback","peer":null,"message_id":null,"actor":p,"data_base64":null,"game":"untrusted"}),
        json!({"kind":"unsupported","constructor":u32::MAX}),
        json!({"kind":"baseline"}),
        json!({"kind":"checkpoint"}),
        json!({"kind":"resync_prepared"}),
        json!({"kind":"gap","peer":p,"reason":"explicit_host_recovery"}),
        json!({"kind":"source_changed"}),
    ] {
        let typed: Event = serde_json::from_value(original).unwrap();
        let serialized = serde_json::to_value(typed).unwrap();
        assert!(validator.is_valid(&serialized), "{serialized}");
        let mut injected = serialized;
        injected["authority"] = json!(true);
        assert!(!validator.is_valid(&injected));
    }
}

#[cfg(feature = "cli")]
#[tokio::test]
async fn broken_cli_output_preserves_the_confirmed_write_and_mt_recovery_advice() {
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    struct Broken;
    impl tokio::io::AsyncWrite for Broken {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    let f = Fixture::new().await;
    let input = serde_json::to_vec(&send("lost-output")).unwrap();
    let error = tg_agent::cli::run_json(
        &f.session,
        input.as_slice(),
        Broken,
        ExecutionMode::Execute,
        tg_agent::tools::OutputLimit::default(),
    )
    .await
    .unwrap_err();
    let error = serde_json::to_value(error).unwrap();
    assert_eq!(error["code"], "agent_io");
    assert_eq!(error["effect_outcome"], "unknown");
    assert_eq!(error["operation_id"], "lost-output");
    assert_eq!(error["next_action"], "mtproto_get_operation");
    let cached = f
        .session
        .dispatch_json(&input, ExecutionMode::Execute)
        .await;
    validate(&send("lost-output"), &serde_json::to_value(cached).unwrap());
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[cfg(feature = "cli")]
#[tokio::test]
async fn bounded_mt_output_allows_following_jsonl_frames_without_replaying() {
    let f = Fixture::new().await;
    let read = request("mtproto_read_updates", json!({}), None);
    let identity = request("mtproto_get_me", json!({}), None);
    let mut input = serde_json::to_vec(&read).unwrap();
    input.push(b'\n');
    input.extend_from_slice(&serde_json::to_vec(&identity).unwrap());
    input.push(b'\n');
    let mut output = Vec::new();
    tg_agent::cli::serve_jsonl(
        &f.session,
        input.as_slice(),
        &mut output,
        ExecutionMode::Execute,
        tg_agent::tools::OutputLimit::new(1024).unwrap(),
    )
    .await
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    let frames = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["error"]["code"], "agent_output_too_large");
    assert!(frames[0]["error"].get("effect_outcome").is_none());
    validate_shape(&read, &frames[0]);
    validate(&identity, &frames[1]);
    f.close().await;
}

#[tokio::test]
async fn owned_timeout_returns_unknown_and_never_replays_the_same_key() {
    let f = Fixture::new().await;
    f.control.pending.store(true, Ordering::SeqCst);
    let request = send("timeout");
    let input = serde_json::to_vec(&request).unwrap();
    let first = f
        .session
        .dispatch_json(&input, ExecutionMode::Execute)
        .await;
    let first = serde_json::to_value(first).unwrap();
    validate(&request, &first);
    assert_eq!(first["result"]["operation"]["state"], "unknown");
    assert_eq!(
        first["result"]["operation"]["failure"]["outcome"],
        "unknown"
    );
    let calls = f.control.calls.load(Ordering::SeqCst);
    let second = f
        .session
        .dispatch_json(&input, ExecutionMode::Execute)
        .await;
    validate(&request, &serde_json::to_value(second).unwrap());
    assert_eq!(f.control.calls.load(Ordering::SeqCst), calls);
    assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn mcp_cancellation_and_eof_leave_the_owned_write_joinable_and_recoverable() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    for cancelled in [false, true] {
        let f = Fixture::new().await;
        f.control.pending.store(true, Ordering::SeqCst);
        let (client_io, server_io) = tokio::io::duplex(262144);
        let server = tokio::spawn(
            tg_agent::mcp::McpServer::new(
                f.session.clone(),
                ExecutionMode::Execute,
                tg_agent::tools::OutputLimit::default(),
            )
            .serve(server_io),
        );
        let (read, mut write) = tokio::io::split(client_io);
        let mut read = BufReader::new(read);
        let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"offline","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
        let mut frame=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mtproto_send_message","arguments":send("cancelled"),"_meta":meta}})).unwrap();
        frame.push(b'\n');
        write.write_all(&frame).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), f.control.entered.notified())
            .await
            .unwrap();
        if cancelled {
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":1}}\n").await.unwrap();
        }
        frame = serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":meta}}),
        )
        .unwrap();
        frame.push(b'\n');
        write.write_all(&frame).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut line = String::new();
                assert!(read.read_line(&mut line).await.unwrap() > 0);
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["id"] == 2 {
                    assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 16);
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.journal
                .get("personal", "host", "cancelled")
                .await
                .unwrap()
                .unwrap()
                .state(),
            tg_agent::journal::OperationState::InFlight
        );
        // In the notification case keep the connection alive through confirmation.
        // EOF in the other case must independently release only the adapter future.
        let mut connection = Some((read, write));
        let mut server = Some(server);
        if !cancelled {
            // Stdin EOF leaves stdout readable. Dropping both halves instead
            // tests a broken output pipe, which correctly fails the transport.
            connection.as_mut().unwrap().1.shutdown().await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), server.take().unwrap())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            drop(connection.take());
        }
        f.control.release.notify_one();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if f.journal
                    .get("personal", "host", "cancelled")
                    .await
                    .unwrap()
                    .unwrap()
                    .state()
                    == tg_agent::journal::OperationState::Succeeded
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if let Some((mut read, mut write)) = connection.take() {
            let mut frame = serde_json::to_vec(
                &json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":meta}}),
            )
            .unwrap();
            frame.push(b'\n');
            write.write_all(&frame).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let mut line = String::new();
                    assert!(read.read_line(&mut line).await.unwrap() > 0);
                    let v: Value = serde_json::from_str(&line).unwrap();
                    assert_ne!(
                        v["id"], 1,
                        "cancelled call must not disclose a late result: {v}"
                    );
                    if v["id"] == 3 {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            let mut late = String::new();
            assert!(
                tokio::time::timeout(Duration::from_millis(200), read.read_line(&mut late))
                    .await
                    .is_err(),
                "cancelled result arrived on the still-open connection: {late}"
            );
            drop(write);
            drop(read);
        }
        if let Some(server) = server {
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        let lookup = request(
            "mtproto_get_operation",
            json!({"operation_id":"cancelled"}),
            None,
        );
        validate(
            &lookup,
            &serde_json::to_value(
                f.session
                    .dispatch_json(
                        &serde_json::to_vec(&lookup).unwrap(),
                        ExecutionMode::Execute,
                    )
                    .await,
            )
            .unwrap(),
        );
        assert_eq!(f.control.writes.load(Ordering::SeqCst), 1);
        f.close().await;
    }
}

#[tokio::test]
async fn actual_denied_and_invalid_outputs_validate_independently_for_every_action() {
    let f = Fixture::new().await;
    f.policy
        .revoke(&Principal::new("host").unwrap(), "personal")
        .unwrap();
    let before = f.control.calls.load(Ordering::SeqCst);
    for request in f.requests() {
        let bytes = serde_json::to_vec(&request).unwrap();
        let denied = serde_json::to_value(
            f.session
                .dispatch_json(&bytes, ExecutionMode::Execute)
                .await,
        )
        .unwrap();
        assert_eq!(denied["ok"], false);
        validate_shape(&request, &denied);
        let invalid = serde_json::to_value(
            f.session
                .dispatch_json(b"{\"principal\":\"PRIVATE\"}", ExecutionMode::Execute)
                .await,
        )
        .unwrap();
        validate_shape(&request, &invalid);
        let mismatch = serde_json::to_value(
            f.session
                .execute_named(
                    "get_me",
                    &ToolRequest::from(request.clone()),
                    ExecutionMode::Execute,
                )
                .await,
        )
        .unwrap();
        validate_shape(&request, &mismatch);
    }
    assert_eq!(f.control.calls.load(Ordering::SeqCst), before);
    f.close().await;
}

#[cfg(feature = "cli")]
#[test]
fn extended_executable_discovery_is_explicit_and_independent_of_host_config() {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_tg-agent");
    for args in [
        vec!["tools", "--mtproto"],
        vec!["schema", "--mtproto"],
        vec!["schema", "request", "--mtproto"],
        vec!["schema", "response", "--mtproto"],
    ] {
        let output = Command::new(binary)
            .args(&args)
            .env_remove("TG_BOT_TOKEN")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if args == ["tools", "--mtproto"] {
            assert_eq!(value["schema_version"], 2);
            assert_eq!(value["tools"].as_array().unwrap().len(), 16);
        }
        if args == ["schema", "--mtproto"] {
            assert_eq!(value["schema_version"], 2);
        }
    }
}
