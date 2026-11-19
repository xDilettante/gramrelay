#![cfg(feature = "mtproto")]
use serde_json::{Value, json};
use tg_agent::{
    executor::{
        Action as BotAction, Budget, ExecutorErrorKind, Grant as BotGrant, PolicyStore, Principal,
    },
    mtproto::{self as mt, Action, Grant, PeerId, PeerKind, PeerScope, Request},
    profiles::ProfileIdentity,
};
fn principal() -> Principal {
    Principal::new("alice").unwrap()
}
fn expected() -> ProfileIdentity {
    ProfileIdentity::mtproto_user(42).unwrap()
}
fn peer(kind: PeerKind) -> PeerId {
    PeerId::new(kind, 7).unwrap()
}
fn request(kind: PeerKind) -> Request {
    parse(
        json!({"schema_version":1,"profile":"primary","operation_id":"intent","call":{"tool":"mtproto_send_message","arguments":{"peer":{"kind":match kind { PeerKind::User=>"user",PeerKind::Chat=>"chat",PeerKind::Channel=>"channel"},"id":"7"},"text":"PRIVATE_TEXT"}}}),
    )
}
fn parse(value: Value) -> Request {
    Request::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn grant() -> Grant {
    Grant::new(expected(), Budget::new("retained-budget").unwrap())
        .unwrap()
        .allow(Action::SendMessage)
        .peers(PeerScope::only([peer(PeerKind::User)]))
}

#[test]
fn shared_policy_never_unions_protocol_or_account_kind_authority() {
    let policy = PolicyStore::default();
    let alice = principal();
    let req = request(PeerKind::User);
    policy
        .replace(
            &alice,
            "primary",
            BotGrant::new(42, Budget::new("budget").unwrap()).allow(BotAction::SendMessage),
        )
        .unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    policy.replace_mtproto(&alice, "primary", grant()).unwrap();
    assert!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, None)
            .is_ok()
    );
    for identity in [
        ProfileIdentity::mtproto_bot(42).unwrap(),
        ProfileIdentity::mtproto_user(43).unwrap(),
        ProfileIdentity::bot_api(42).unwrap(),
    ] {
        assert_eq!(
            policy
                .dry_run_mtproto(&alice, &req, identity, 100, None)
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Denied
        );
    }
    let bot_req = tg_agent::executor::Request::from_json(
        br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":{}}}"#,
    )
    .unwrap();
    assert_eq!(
        policy
            .dry_run(&alice, &bot_req, 42, 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    policy.revoke(&alice, "primary").unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
}

#[test]
fn peer_kinds_files_expiry_and_callback_capabilities_are_not_inferred() {
    let policy = PolicyStore::default();
    let alice = principal();
    policy
        .replace_mtproto(&alice, "primary", grant().expires_at_ms(200))
        .unwrap();
    for kind in [PeerKind::Chat, PeerKind::Channel] {
        assert_eq!(
            policy
                .dry_run_mtproto(&alice, &request(kind), expected(), 100, None)
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Denied
        );
    }
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &request(PeerKind::User), expected(), 200, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    let doc = parse(
        json!({"schema_version":1,"profile":"primary","operation_id":"doc","call":{"tool":"mtproto_send_document","arguments":{"peer":{"kind":"user","id":"7"},"file_handle":"approved"}}}),
    );
    policy
        .replace_mtproto(&alice, "primary", grant().allow(Action::SendDocument))
        .unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &doc, expected(), 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    policy
        .replace_mtproto(
            &alice,
            "primary",
            grant().allow(Action::SendDocument).files(["approved"]),
        )
        .unwrap();
    policy
        .dry_run_mtproto(&alice, &doc, expected(), 100, None)
        .unwrap();
    assert_eq!(
        policy
            .replace_mtproto(
                &alice,
                "primary",
                grant().allow(Action::AnswerCallbackQuery)
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    assert!(
        Grant::new(
            ProfileIdentity::bot_api(42).unwrap(),
            Budget::new("b").unwrap()
        )
        .is_err()
    );
    let bot = ProfileIdentity::mtproto_bot(42).unwrap();
    policy
        .replace_mtproto(
            &alice,
            "primary",
            Grant::new(bot, Budget::new("b").unwrap())
                .unwrap()
                .allow(Action::AnswerCallbackQuery),
        )
        .unwrap();
    let callback = parse(
        json!({"schema_version":1,"profile":"primary","operation_id":"cb","call":{"tool":"mtproto_answer_callback_query","arguments":{"reference":"observed"}}}),
    );
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &callback, bot, 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ContextRequired
    );
}

#[test]
fn exact_approvals_expire_and_cannot_survive_protocol_replacement() {
    let policy = PolicyStore::default();
    let alice = principal();
    let req = request(PeerKind::User);
    let requiring = || grant().require_approval(Action::SendMessage);
    policy
        .replace_mtproto(&alice, "primary", requiring())
        .unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, None)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = policy
        .approve_mtproto(&alice, &req, expected(), 100, 200)
        .unwrap();
    policy
        .dry_run_mtproto(&alice, &req, expected(), 100, Some(&approval))
        .unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 200, Some(&approval))
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let mut changed = serde_json::to_value(&req).unwrap();
    changed["operation_id"] = json!("other");
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &parse(changed), expected(), 100, Some(&approval))
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let other = PolicyStore::default();
    other
        .replace_mtproto(&alice, "primary", requiring())
        .unwrap();
    assert_eq!(
        other
            .dry_run_mtproto(&alice, &req, expected(), 100, Some(&approval))
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    policy
        .replace(
            &alice,
            "primary",
            BotGrant::new(42, Budget::new("b").unwrap()),
        )
        .unwrap();
    policy
        .replace_mtproto(&alice, "primary", requiring())
        .unwrap();
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, Some(&approval))
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
}

#[test]
fn typed_input_has_canonical_peer_domains_and_redacted_fixed_shape() {
    assert_eq!(Action::ALL.len(), 8);
    let req = request(PeerKind::Channel);
    assert!(!format!("{req:?} {:?}", req.call()).contains("PRIVATE_TEXT"));
    assert_eq!(
        req.call().peer().unwrap().as_client_id(),
        tg_mtproto::PeerId::channel(7).unwrap()
    );
    for value in [
        json!(0),
        json!("0"),
        json!("+7"),
        json!("07"),
        json!("-7"),
        json!("9223372036854775807"),
    ] {
        let mut wire = serde_json::to_value(&req).unwrap();
        wire["call"]["arguments"]["peer"]["id"] = value;
        assert!(Request::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    for (path, value) in [
        ("peer", json!(["channel", "7"])),
        ("text", json!("😀".repeat(2049))),
        ("api_hash", json!("PRIVATE_KEY")),
    ] {
        let mut wire = serde_json::to_value(&req).unwrap();
        wire["call"]["arguments"][path] = value;
        assert!(Request::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    assert!(Request::from_json(br#"{"schema_version":1,"profile":"primary","profile":"other","call":{"tool":"mtproto_get_me","arguments":{}}}"#).is_err());
    assert!(Request::from_json(br#"{"schema_version":1,"profile":"primary","operation_id":null,"call":{"tool":"mtproto_get_me","arguments":{}}}"#).is_err());
    let _ = mt::Call::GetMe(tg_agent::executor::EmptyArgs {});
}

#[test]
fn all_eight_intents_round_trip_without_bot_options_or_dynamic_dispatch() {
    let p = json!({"kind":"user","id":"7"});
    let fixtures = [
        (Action::GetMe, json!({})),
        (Action::GetPeer, json!({"peer":p})),
        (Action::ReadUpdates, json!({"limit":20})),
        (Action::SendMessage, json!({"peer":p,"text":"text"})),
        (
            Action::EditMessageText,
            json!({"peer":p,"message_id":"1","text":"text"}),
        ),
        (Action::AnswerCallbackQuery, json!({"reference":"observed"})),
        (Action::GetOperation, json!({"operation_id":"prior"})),
        (
            Action::SendDocument,
            json!({"peer":p,"file_handle":"approved"}),
        ),
    ];
    for (action, arguments) in fixtures {
        assert!(
            !tg_agent::executor::Action::ALL
                .iter()
                .any(|a| a.name() == action.name())
        );
        let mut wire = json!({"schema_version":1,"profile":"primary","call":{"tool":action.name(),"arguments":arguments}});
        if action.is_write() {
            wire["operation_id"] = json!("intent");
        }
        let parsed = parse(wire);
        assert_eq!(parsed.action(), action);
        assert_eq!(serde_json::to_value(action).unwrap(), action.name());
        let canonical = serde_json::to_vec(&parsed).unwrap();
        assert_eq!(
            serde_json::to_vec(&Request::from_json(&canonical).unwrap()).unwrap(),
            canonical
        );
    }
    for wire in [
        json!({"schema_version":2,"profile":"primary","call":{"tool":"mtproto_get_me","arguments":{}}}),
        json!({"schema_version":1,"profile":"primary","call":{"tool":"mtproto_get_me","arguments":[]}}),
        json!({"schema_version":1,"profile":"primary","call":{"tool":"raw_call","arguments":{}}}),
        json!({"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":{}}}),
        json!({"schema_version":1,"profile":"primary","operation_id":"edit","call":{"tool":"mtproto_edit_message_text","arguments":{"peer":p,"message_id":"2147483648","text":"text"}}}),
        json!({"schema_version":1,"profile":"primary","operation_id":"send","call":{"tool":"mtproto_send_message","arguments":{"peer":p,"text":"text","message_thread_id":"1"}}}),
    ] {
        assert!(Request::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    let padded = format!(
        "{}{}",
        " ".repeat(Request::MAX_JSON_BYTES),
        serde_json::to_string(&request(PeerKind::User)).unwrap()
    );
    assert!(Request::from_json(padded.as_bytes()).is_err());
}

#[test]
fn invalid_grants_are_rejected_before_replacing_a_current_revision() {
    let policy = PolicyStore::default();
    let alice = principal();
    let req = request(PeerKind::User);
    policy
        .replace_mtproto(
            &alice,
            "primary",
            grant().require_approval(Action::SendMessage),
        )
        .unwrap();
    let approval = policy
        .approve_mtproto(&alice, &req, expected(), 100, 200)
        .unwrap();
    for invalid in [
        grant().require_approval(Action::GetMe),
        grant().expires_at_ms(0),
        grant().action_cost(Action::SendMessage, u64::MAX),
        grant().files(["bad handle"]),
    ] {
        assert_eq!(
            policy
                .replace_mtproto(&alice, "primary", invalid)
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::InvalidPolicy
        );
        policy
            .dry_run_mtproto(&alice, &req, expected(), 100, Some(&approval))
            .unwrap();
    }
    assert_eq!(
        policy
            .dry_run_mtproto(&alice, &req, expected(), -1, Some(&approval))
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
    assert_eq!(
        policy
            .approve_mtproto(&alice, &req, expected(), 100, 100)
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::InvalidPolicy
    );
}

#[test]
fn direct_serde_request_and_peer_errors_do_not_echo_untrusted_names() {
    for wire in [
        json!({"schema_version":1,"profile":"primary","PRIVATE_TOKEN":"secret","call":{"tool":"mtproto_get_me","arguments":{}}}),
        json!({"schema_version":1,"profile":"primary","call":{"tool":"PRIVATE_TOKEN","arguments":{}}}),
        json!({"schema_version":1,"profile":"primary","call":{"tool":"mtproto_get_peer","arguments":{"peer":{"kind":"PRIVATE_TOKEN","id":"7"}}}}),
    ] {
        let error =
            serde_json::from_slice::<Request>(&serde_json::to_vec(&wire).unwrap()).unwrap_err();
        assert!(!format!("{error} {error:?}").contains("PRIVATE_TOKEN"));
    }
    for wire in [
        json!({"kind":"PRIVATE_TOKEN","id":"7"}),
        json!({"kind":"user","id":"7","PRIVATE_TOKEN":"secret"}),
    ] {
        let error =
            serde_json::from_slice::<PeerId>(&serde_json::to_vec(&wire).unwrap()).unwrap_err();
        assert!(!format!("{error} {error:?}").contains("PRIVATE_TOKEN"));
    }
}
