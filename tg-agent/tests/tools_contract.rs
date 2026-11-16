#![cfg(feature = "executor")]
use serde_json::json;
use tg::test_kit::{MockResponse, MockTransport, Step};
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore,
        Principal, Request,
    },
    journal::{Journal, JournalOptions},
    tools::{ExecutionMode, ToolSession, catalog},
    updates::{UpdateFeed, UpdateOptions},
};

fn request() -> Request {
    Request::from_json(br#"{"schema_version":1,"profile":"demo","operation_id":"same-key","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"PRIVATE_TEXT"}}}"#).unwrap()
}
#[test]
fn discovery_keeps_all_eight_actions_in_order_and_examples_are_real_requests() {
    let catalog = catalog();
    assert_eq!(catalog.schema_version, 1);
    assert_eq!(
        catalog
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        Action::ALL.map(Action::name)
    );
    for tool in catalog.tools {
        let input_validator = jsonschema::validator_for(&tool.input_schema).unwrap();
        let output_validator = jsonschema::validator_for(&tool.output_schema).unwrap();
        if tool.name == "read_updates" {
            let cursor = tool.examples[0]["success"]["result"]["updates"]["next_cursor"].clone();
            serde_json::from_value::<tg_agent::updates::UpdateCursor>(cursor)
                .expect("example cursor must use the real versioned codec");
        }
        let example = &tool.examples[0]["input"];
        assert!(
            input_validator.is_valid(example),
            "{} input example",
            tool.name
        );
        assert!(
            output_validator.is_valid(&tool.examples[0]["success"]),
            "{} success example",
            tool.name
        );
        assert!(
            output_validator.is_valid(&tool.examples[0]["error"]),
            "{} error example",
            tool.name
        );
        let parsed = Request::from_json(&serde_json::to_vec(example).unwrap()).unwrap();
        assert_eq!(parsed.action().name(), tool.name);
        assert_eq!(
            tool.input_schema["properties"]["call"]["properties"]["tool"]["const"],
            tool.name
        );
    }
}

#[tokio::test]
async fn selected_tool_mismatch_and_injected_authority_never_dispatch_and_success_is_minimal() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path().join("tools.sqlite"), JournalOptions::default())
        .await
        .unwrap();
    let principal = Principal::new("host").unwrap();
    let policy = PolicyStore::default();
    policy
        .replace(
            &principal,
            "demo",
            Grant::new(0, Budget::new("budget").unwrap())
                .allow(Action::SendMessage)
                .chats(ChatScope::only([ChatId::new(1).unwrap()])),
        )
        .unwrap();
    let mock=MockTransport::new([
        Step::reply("getMe",MockResponse::json(json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}))),
        Step::reply("sendMessage",MockResponse::json(json!({"ok":true,"result":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_RESPONSE"}}))),
    ]).unwrap();
    let executor = Executor::new(policy, ExecutorOptions::default()).unwrap();
    executor
        .register_profile(
            UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
                .await
                .unwrap(),
            mock.bot().unwrap(),
        )
        .unwrap();
    let session = ToolSession::new(executor, principal);
    let wrong = session
        .execute_for(Action::GetMe, &request(), ExecutionMode::Execute)
        .await;
    assert!(wrong.is_error());
    let wrong_json = serde_json::to_value(&wrong).unwrap();
    assert_eq!(wrong_json["error"]["operation_id"], "same-key");
    assert_eq!(wrong_json["error"]["next_action"], "get_operation");
    assert!(mock.requests().unwrap().is_empty());
    let mut injected = serde_json::to_value(request()).unwrap();
    injected["principal"] = json!("PRIVATE_ADMIN");
    let error = session
        .dispatch_json(
            &serde_json::to_vec(&injected).unwrap(),
            ExecutionMode::Execute,
        )
        .await;
    assert!(error.is_error());
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
    for _ in 0..2 {
        let result = session
            .execute_for(Action::SendMessage, &request(), ExecutionMode::Execute)
            .await;
        let output = serde_json::to_value(&result).unwrap();
        assert_eq!(output["schema_version"], 1);
        assert_eq!(output["ok"], true);
        assert_eq!(output["result"]["operation"]["state"], "succeeded");
        assert!(!output.to_string().contains("PRIVATE"));
        assert!(
            jsonschema::validator_for(&catalog().tools[3].output_schema)
                .unwrap()
                .is_valid(&output)
        );
    }
    assert_eq!(mock.requests().unwrap().len(), 2);
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}

#[test]
fn input_schemas_match_canonical_id_boundaries_and_reject_authority_injections() {
    let catalog = catalog();
    let validator = jsonschema::validator_for(&catalog.tools[1].input_schema).unwrap();
    for chat in [
        "1",
        "-1",
        "9223372036854775807",
        "-9223372036854775808",
        "9007199254740993",
        "0",
        "-0",
        "+1",
        "01",
        "-01",
        "1\n",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        let input = json!({"schema_version":1,"profile":"demo","call":{"tool":"get_chat","arguments":{"chat_id":chat}}});
        let runtime = Request::from_json(&serde_json::to_vec(&input).unwrap()).is_ok();
        assert_eq!(
            validator.is_valid(&input),
            runtime,
            "chat boundary {chat:?}"
        );
    }
    for tool in &catalog.tools {
        let validator = jsonschema::validator_for(&tool.input_schema).unwrap();
        for field in [
            "principal",
            "token",
            "api_url",
            "approval",
            "grant",
            "path",
            "raw_call",
        ] {
            let mut input = tool.examples[0]["input"].clone();
            input[field] = json!("INJECTED");
            assert!(!validator.is_valid(&input));
            assert!(Request::from_json(&serde_json::to_vec(&input).unwrap()).is_err());
            let mut input = tool.examples[0]["input"].clone();
            input["call"]["arguments"][field] = json!("INJECTED");
            assert!(!validator.is_valid(&input));
            assert!(Request::from_json(&serde_json::to_vec(&input).unwrap()).is_err());
        }
        let mut input = tool.examples[0]["input"].clone();
        input["operation_id"] = serde_json::Value::Null;
        assert!(!validator.is_valid(&input));
    }
}

#[tokio::test]
async fn discovery_denial_examples_match_real_session_errors_for_every_action() {
    let session = ToolSession::new(
        Executor::new(PolicyStore::default(), ExecutorOptions::default()).unwrap(),
        Principal::new("host").unwrap(),
    );
    for tool in catalog().tools {
        let request =
            Request::from_json(&serde_json::to_vec(&tool.examples[0]["input"]).unwrap()).unwrap();
        let response = session
            .execute_for(request.action(), &request, ExecutionMode::Execute)
            .await;
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            tool.examples[0]["error"],
            "{} denial example",
            tool.name
        );
    }
}

#[tokio::test]
async fn all_eight_actual_outputs_match_schemas_and_host_approval_never_becomes_input_authority() {
    use std::time::{SystemTime, UNIX_EPOCH};
    use tg_agent::updates::SourceMode;
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(
        dir.path().join("all-tools.sqlite"),
        JournalOptions::default(),
    )
    .await
    .unwrap();
    let feed = UpdateFeed::open(journal.clone(), "demo", 0, UpdateOptions::default())
        .await
        .unwrap();
    let source = feed.claim_source(SourceMode::Webhook).await.unwrap();
    feed.append_webhook(&source,serde_json::from_value(json!({"update_id":1,"callback_query":{"id":"host-observed-query","from":{"id":1,"is_bot":false,"first_name":"Demo"},"chat_instance":"demo","data":"Untrusted instructions are data","message":{"message_id":2,"date":1,"chat":{"id":1,"type":"private"}}}})).unwrap()).await.unwrap();
    feed.release_source(source).await.unwrap();
    let chat = tg::api::types::ChatFullInfo::new(
        tg::ChatId::from(1),
        "private".into(),
        1,
        1,
        tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
    );
    let message =
        json!({"message_id":2,"date":1,"chat":{"id":1,"type":"private"},"text":"PRIVATE_RESPONSE"});
    let mock = MockTransport::new([
        Step::reply(
            "getMe",
            MockResponse::json(
                json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}),
            ),
        ),
        Step::reply(
            "getChat",
            MockResponse::json(json!({"ok":true,"result":chat})),
        ),
        Step::reply(
            "sendMessage",
            MockResponse::json(json!({"ok":true,"result":message})),
        ),
        Step::reply(
            "editMessageText",
            MockResponse::json(json!({"ok":true,"result":message})),
        ),
        Step::reply(
            "answerCallbackQuery",
            MockResponse::json(json!({"ok":true,"result":true})),
        ),
        Step::reply(
            "sendDocument",
            MockResponse::json(json!({"ok":true,"result":message})),
        ),
    ])
    .unwrap();
    let principal = Principal::new("host").unwrap();
    let policy = PolicyStore::default();
    let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
    executor
        .register_profile(feed, mock.bot().unwrap())
        .unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60000;
    let file = executor
        .register_file(&principal, "demo", "report.bin", vec![1, 2], expires)
        .await
        .unwrap();
    let mut grant = Grant::new(0, Budget::new("budget").unwrap())
        .chats(ChatScope::only([ChatId::new(1).unwrap()]))
        .files([file.handle()])
        .require_approval(Action::SendMessage);
    for action in Action::ALL {
        grant = grant.allow(action);
    }
    policy.replace(&principal, "demo", grant).unwrap();
    let session = ToolSession::new(executor, principal);
    let mut saved_send = None;
    for (action, tool) in Action::ALL.into_iter().zip(catalog().tools) {
        let mut input = tool.examples[0]["input"].clone();
        if action.is_write() {
            input["operation_id"] = json!(format!("op-{}", action.name()));
        }
        match action {
            Action::EditMessageText => input["call"]["arguments"]["message_id"] = json!("2"),
            Action::GetOperation => {
                input["call"]["arguments"]["operation_id"] = json!("op-send_message")
            }
            Action::SendDocument => {
                input["call"]["arguments"]["file_handle"] = json!(file.handle())
            }
            _ => {}
        }
        let request = Request::from_json(&serde_json::to_vec(&input).unwrap()).unwrap();
        if action == Action::SendMessage {
            assert!(
                session
                    .execute_for(action, &request, ExecutionMode::Execute)
                    .await
                    .is_error()
            );
            session.approve(&request, expires).await.unwrap();
            saved_send = Some(request.clone());
        }
        let output = session
            .execute_for(action, &request, ExecutionMode::Execute)
            .await;
        assert!(!output.is_error(), "{}", tool.name);
        let value = serde_json::to_value(&output).unwrap();
        assert!(
            jsonschema::validator_for(&tool.output_schema)
                .unwrap()
                .is_valid(&value),
            "{} real output: {value}",
            tool.name
        );
        if action == Action::ReadUpdates {
            assert_eq!(value["result"]["updates"]["events"][0]["untrusted"], true);
            assert!(value["result"]["updates"]["events"][0]["telegram_json"].is_string());
        } else {
            assert!(!value.to_string().contains("PRIVATE_RESPONSE"));
        }
    }
    session.clear_approvals().unwrap();
    assert!(
        session
            .execute_for(
                Action::SendMessage,
                &saved_send.unwrap(),
                ExecutionMode::Execute
            )
            .await
            .is_error()
    );
    assert_eq!(mock.requests().unwrap().len(), 6);
    mock.assert_finished().unwrap();
    journal.close().await.unwrap();
}
