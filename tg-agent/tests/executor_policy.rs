#![cfg(feature = "executor")]

use serde_json::{Value, json};
use tg_agent::executor::{
    Action, Budget, ChatId, ChatScope, ExecutorErrorKind, Grant, PolicyStore, Principal, Request,
};

fn call(tool: &str, arguments: Value) -> Value {
    let mut value =
        json!({"schema_version":1,"profile":"primary","call":{"tool":tool,"arguments":arguments}});
    if [
        "send_message",
        "edit_message_text",
        "answer_callback_query",
        "send_document",
    ]
    .contains(&tool)
    {
        value["operation_id"] = json!("stable-operation");
    }
    value
}
fn parse(value: Value) -> Request {
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn alice() -> Principal {
    Principal::new("alice").unwrap()
}
fn grant() -> Grant {
    Grant::new(123, Budget::new("stable-budget").unwrap())
        .allow(Action::SendMessage)
        .chats(ChatScope::only([ChatId::new(-100123).unwrap()]))
}

#[test]
fn stable_inventory_and_json_ids_keep_all_signed_64_bit_precision() {
    assert_eq!(
        Action::ALL.map(Action::name),
        [
            "get_me",
            "get_chat",
            "read_updates",
            "send_message",
            "edit_message_text",
            "answer_callback_query",
            "get_operation",
            "send_document"
        ]
    );
    for id in [i64::MIN, -100123, 1, i64::MAX] {
        let request = parse(call("get_chat", json!({"chat_id":id.to_string()})));
        assert_eq!(
            serde_json::to_value(&request).unwrap()["call"]["arguments"]["chat_id"],
            id.to_string()
        );
    }
    for id in [
        json!(123),
        json!("0"),
        json!("+1"),
        json!("01"),
        json!("-0"),
        json!(" 1"),
        json!("@channel"),
        json!("9223372036854775808"),
    ] {
        assert!(
            Request::from_json(
                &serde_json::to_vec(&call("get_chat", json!({"chat_id":id}))).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn input_cannot_inject_identity_approval_config_or_unreviewed_methods() {
    let valid = call(
        "send_message",
        json!({"chat_id":"-100123","text":"PRIVATE_TEXT"}),
    );
    for name in [
        "principal",
        "token",
        "api_url",
        "approval",
        "grants",
        "dry_run",
    ] {
        let mut value = valid.clone();
        value[name] = json!("PRIVATE_INJECTION");
        let error = Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert_eq!(error.kind(), ExecutorErrorKind::InvalidInput);
        assert!(!format!("{error:?} {error}").contains("PRIVATE_"));
    }
    for name in [
        "allow_paid_broadcast",
        "parse_mode",
        "chat_scope",
        "principal",
    ] {
        let mut value = valid.clone();
        value["call"]["arguments"][name] = json!(true);
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for tool in ["raw_call", "delete_message", "SEND_MESSAGE"] {
        assert!(Request::from_json(&serde_json::to_vec(&call(tool, json!({}))).unwrap()).is_err());
    }
    let request = parse(valid);
    assert!(!format!("{request:?}").contains("PRIVATE_TEXT"));
}

#[test]
fn required_operation_keys_versions_limits_and_callback_file_boundaries_are_strict() {
    let mut value = call("send_message", json!({"chat_id":"-100123","text":"ok"}));
    value.as_object_mut().unwrap().remove("operation_id");
    assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = call("get_me", json!({}));
    value["operation_id"] = json!("unexpected");
    assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = call("get_me", json!({}));
    value["schema_version"] = json!(2);
    assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    for args in [
        json!({"chat_id":"1","text":""}),
        json!({"chat_id":"1","text":"a".repeat(4097)}),
        json!({"chat_id":"1","text":"ok","message_thread_id":"0"}),
    ] {
        assert!(
            Request::from_json(&serde_json::to_vec(&call("send_message", args)).unwrap()).is_err()
        );
    }
    assert!(Request::from_json(&vec![b' '; 65537]).is_err());
    assert!(
        Request::from_json(
            &serde_json::to_vec(&call("read_updates", json!({"limit":101}))).unwrap()
        )
        .is_err()
    );
    assert!(
        Request::from_json(
            &serde_json::to_vec(&call(
                "answer_callback_query",
                json!({"callback_query_id":"opaque","chat_id":"1"})
            ))
            .unwrap()
        )
        .is_err()
    );
    for args in [
        json!({"chat_id":"1","path":"C:/private"}),
        json!({"chat_id":"1","url":"https://invalid.example/private"}),
        json!({"chat_id":"1","file_id":"opaque"}),
    ] {
        assert!(
            Request::from_json(&serde_json::to_vec(&call("send_document", args)).unwrap()).is_err()
        );
    }
}

#[test]
fn policy_defaults_deny_and_current_bot_action_profile_chat_and_expiry_are_checked() {
    let policy = PolicyStore::default();
    let request = parse(call(
        "send_message",
        json!({"chat_id":"-100123","text":"ok"}),
    ));
    assert_eq!(
        policy
            .dry_run(&alice(), &request, 123, 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    policy
        .replace(&alice(), "primary", grant().expires_at_ms(200))
        .unwrap();
    assert!(policy.dry_run(&alice(), &request, 123, 100, None).is_ok());
    assert!(policy.dry_run(&alice(), &request, 124, 100, None).is_err());
    assert!(policy.dry_run(&alice(), &request, 123, 200, None).is_err());
    assert!(
        policy
            .dry_run(&Principal::new("other").unwrap(), &request, 123, 100, None)
            .is_err()
    );
    for args in [
        json!({"chat_id":"1","text":"ok"}),
        json!({"chat_id":"-100124","text":"ok"}),
    ] {
        assert!(
            policy
                .dry_run(&alice(), &parse(call("send_message", args)), 123, 100, None)
                .is_err()
        );
    }
    assert!(
        policy
            .dry_run(&alice(), &parse(call("get_me", json!({}))), 123, 100, None)
            .is_err()
    );
    policy.revoke(&alice(), "primary").unwrap();
    assert!(policy.dry_run(&alice(), &request, 123, 100, None).is_err());
}

#[test]
fn host_approval_is_bound_to_request_principal_policy_store_revision_and_expiry() {
    let policy = PolicyStore::default();
    let confirmed = grant().require_approval(Action::SendMessage);
    policy
        .replace(&alice(), "primary", confirmed.clone())
        .unwrap();
    let request = parse(call(
        "send_message",
        json!({"chat_id":"-100123","text":"ok"}),
    ));
    assert_eq!(
        policy
            .dry_run(&alice(), &request, 123, 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = policy.approve(&alice(), &request, 123, 100, 200).unwrap();
    assert!(
        policy
            .dry_run(&alice(), &request, 123, 100, Some(&approval))
            .is_ok()
    );
    assert!(
        policy
            .dry_run(&alice(), &request, 123, 200, Some(&approval))
            .is_err()
    );
    let changed = parse(call(
        "send_message",
        json!({"chat_id":"-100123","text":"different"}),
    ));
    assert!(
        policy
            .dry_run(&alice(), &changed, 123, 100, Some(&approval))
            .is_err()
    );
    let other = PolicyStore::default();
    other
        .replace(&alice(), "primary", confirmed.clone())
        .unwrap();
    assert!(
        other
            .dry_run(&alice(), &request, 123, 100, Some(&approval))
            .is_err()
    );
    policy.replace(&alice(), "primary", confirmed).unwrap();
    assert!(
        policy
            .dry_run(&alice(), &request, 123, 100, Some(&approval))
            .is_err()
    );
    assert!(!format!("{approval:?} {policy:?}").contains("-100123"));
}

#[test]
fn file_grants_are_intersected_with_chat_scope_and_unresolved_references_fail_closed() {
    let policy = PolicyStore::default();
    policy
        .replace(
            &alice(),
            "primary",
            grant()
                .allow(Action::SendDocument)
                .allow(Action::AnswerCallbackQuery)
                .allow(Action::GetOperation)
                .files(["file-one"]),
        )
        .unwrap();
    let allowed = parse(call(
        "send_document",
        json!({"chat_id":"-100123","file_handle":"file-one"}),
    ));
    assert!(policy.dry_run(&alice(), &allowed, 123, 100, None).is_ok());
    for args in [
        json!({"chat_id":"1","file_handle":"file-one"}),
        json!({"chat_id":"-100123","file_handle":"file-two"}),
    ] {
        assert!(
            policy
                .dry_run(
                    &alice(),
                    &parse(call("send_document", args)),
                    123,
                    100,
                    None
                )
                .is_err()
        );
    }
    for request in [
        parse(call(
            "answer_callback_query",
            json!({"callback_query_id":"opaque"}),
        )),
        parse(call("get_operation", json!({"operation_id":"previous"}))),
    ] {
        assert_eq!(
            policy
                .dry_run(&alice(), &request, 123, 100, None)
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::ContextRequired
        );
    }
}

#[test]
fn null_operation_fields_and_duplicate_or_extra_envelope_fields_fail_closed() {
    let mut read = call("get_me", json!({}));
    read["operation_id"] = Value::Null;
    assert!(Request::from_json(&serde_json::to_vec(&read).unwrap()).is_err());
    let mut extra = call("get_me", json!({}));
    extra["call"]["principal"] = json!("PRIVATE_PRINCIPAL");
    assert!(Request::from_json(&serde_json::to_vec(&extra).unwrap()).is_err());
    assert!(Request::from_json(br#"{"schema_version":1,"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":{}}}"#).is_err());
    assert!(Request::from_json(br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_chat","arguments":{"chat_id":"1","chat_id":"2"}}}"#).is_err());
}

#[test]
fn approval_cannot_move_to_another_operation_principal_profile_or_recreated_grant() {
    let policy = PolicyStore::default();
    let confirmed = grant().require_approval(Action::SendMessage);
    let bob = Principal::new("bob").unwrap();
    policy
        .replace(&alice(), "primary", confirmed.clone())
        .unwrap();
    policy.replace(&bob, "primary", confirmed.clone()).unwrap();
    policy
        .replace(&alice(), "secondary", confirmed.clone())
        .unwrap();
    let value = call("send_message", json!({"chat_id":"-100123","text":"ok"}));
    let request = parse(value.clone());
    let approval = policy.approve(&alice(), &request, 123, 100, 200).unwrap();
    let mut different_key = value.clone();
    different_key["operation_id"] = json!("second-write");
    assert!(
        policy
            .dry_run(&alice(), &parse(different_key), 123, 100, Some(&approval))
            .is_err()
    );
    assert!(
        policy
            .dry_run(&bob, &request, 123, 100, Some(&approval))
            .is_err()
    );
    let mut different_profile = value;
    different_profile["profile"] = json!("secondary");
    assert!(
        policy
            .dry_run(
                &alice(),
                &parse(different_profile),
                123,
                100,
                Some(&approval)
            )
            .is_err()
    );
    policy.revoke(&alice(), "primary").unwrap();
    policy.replace(&alice(), "primary", confirmed).unwrap();
    assert!(
        policy
            .dry_run(&alice(), &request, 123, 100, Some(&approval))
            .is_err()
    );
}

#[test]
fn grant_validation_and_explicit_global_scope_do_not_silently_widen_permissions() {
    use std::time::Duration;
    use tg_agent::executor::BudgetLimits;
    let policy = PolicyStore::default();
    let request = parse(call("get_chat", json!({"chat_id":"1"})));
    let no_chats = Grant::new(123, Budget::new("quota").unwrap()).allow(Action::GetChat);
    policy
        .replace(&alice(), "primary", no_chats.clone())
        .unwrap();
    assert!(policy.dry_run(&alice(), &request, 123, 100, None).is_err());
    let all = no_chats.chats(ChatScope::All);
    assert!(!all.permits_unscoped_updates());
    policy.replace(&alice(), "primary", all).unwrap();
    assert!(policy.dry_run(&alice(), &request, 123, 100, None).is_ok());
    let invalid = [
        Grant::new(-1, Budget::new("q").unwrap()),
        Grant::new(123, Budget::new("q").unwrap().window(Duration::ZERO)),
        Grant::new(
            123,
            Budget::new("q").unwrap().limits(BudgetLimits {
                invocations: u64::MAX,
                ..BudgetLimits::default()
            }),
        ),
        grant().expires_at_ms(0),
        grant().require_approval(Action::GetChat),
        grant().files(["../private"]),
    ];
    for value in invalid {
        assert!(policy.replace(&alice(), "primary", value).is_err());
    }
    // A failed replacement leaves the previous complete grant intact.
    assert!(policy.dry_run(&alice(), &request, 123, 100, None).is_ok());
}

#[test]
fn positional_array_aliases_are_not_the_versioned_object_schema() {
    for bytes in [
        br#"[1,"primary",null,{"tool":"get_me","arguments":{}}]"#.as_slice(),
        br#"{"schema_version":1,"profile":"primary","call":["get_chat",["1"]]}"#,
        br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_chat","arguments":["1"]}}"#,
        br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":[]}}"#,
    ] {
        assert!(Request::from_json(bytes).is_err());
    }
}
