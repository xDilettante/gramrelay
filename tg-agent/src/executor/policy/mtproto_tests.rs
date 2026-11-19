use super::*;
use crate::mtproto::{Call, PeerId, PeerKind, PeerScope};
fn expected() -> ProfileIdentity {
    ProfileIdentity::mtproto_user(42).unwrap()
}
fn request() -> Request {
    Request::new(
        "primary",
        None,
        Call::GetOperation(crate::executor::GetOperationArgs {
            operation_id: "retained".into(),
        }),
    )
    .unwrap()
}
fn grant() -> Grant {
    Grant::new(expected(), Budget::new("retained").unwrap())
        .unwrap()
        .allow(Action::GetOperation)
        .allow(Action::SendDocument)
        .require_approval(Action::SendDocument)
        .peers(PeerScope::only([PeerId::new(PeerKind::User, 7).unwrap()]))
        .files(["approved"])
}
fn scope() -> OriginalScope {
    OriginalScope {
        account: expected(),
        action: Action::SendDocument,
        peer: Some(PeerId::new(PeerKind::User, 7).unwrap()),
        file_handle: Some("approved".into()),
    }
}
#[test]
fn retained_scope_checks_current_original_resources_without_reconfirming_the_write() {
    let policy = PolicyStore::default();
    let alice = Principal::new("alice").unwrap();
    let req = request();
    let original = scope();
    policy.replace_mtproto(&alice, "primary", grant()).unwrap();
    assert!(
        policy
            .authorized_mtproto(
                &alice,
                &req,
                expected(),
                PolicyTime::Fixed(100),
                None,
                Some(&original)
            )
            .is_ok()
    );
    let mut wrong_account = scope();
    wrong_account.account = ProfileIdentity::mtproto_bot(42).unwrap();
    let mut wrong_peer = scope();
    wrong_peer.peer = Some(PeerId::new(PeerKind::Channel, 7).unwrap());
    let mut wrong_file = scope();
    wrong_file.file_handle = Some("other".into());
    for wrong in [wrong_account, wrong_peer, wrong_file] {
        assert_eq!(
            policy
                .authorized_mtproto(
                    &alice,
                    &req,
                    expected(),
                    PolicyTime::Fixed(100),
                    None,
                    Some(&wrong)
                )
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Denied
        );
    }
    policy
        .replace_mtproto(
            &alice,
            "primary",
            grant().require_approval(Action::GetOperation),
        )
        .unwrap();
    assert_eq!(
        policy
            .authorized_mtproto(
                &alice,
                &req,
                expected(),
                PolicyTime::Fixed(100),
                None,
                Some(&original)
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::ApprovalRequired
    );
    let approval = policy
        .approve_mtproto_with_scope(
            &alice,
            &req,
            expected(),
            PolicyTime::Fixed(100),
            200,
            Some(&original),
        )
        .unwrap();
    policy
        .authorized_mtproto(
            &alice,
            &req,
            expected(),
            PolicyTime::Fixed(100),
            Some(&approval),
            Some(&original),
        )
        .unwrap();
    policy
        .replace_mtproto(
            &alice,
            "primary",
            Grant::new(expected(), Budget::new("retained").unwrap())
                .unwrap()
                .allow(Action::GetOperation)
                .peers(PeerScope::All)
                .files(["approved"]),
        )
        .unwrap();
    assert_eq!(
        policy
            .authorized_mtproto(
                &alice,
                &req,
                expected(),
                PolicyTime::Fixed(100),
                None,
                Some(&original)
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
}
#[test]
fn global_callback_scope_requires_explicit_bot_and_unscoped_authority() {
    let policy = PolicyStore::default();
    let alice = Principal::new("alice").unwrap();
    let req = request();
    let bot = ProfileIdentity::mtproto_bot(42).unwrap();
    let grant = || {
        Grant::new(bot, Budget::new("b").unwrap())
            .unwrap()
            .allow(Action::GetOperation)
            .allow(Action::AnswerCallbackQuery)
    };
    let scope = OriginalScope {
        account: bot,
        action: Action::AnswerCallbackQuery,
        peer: None,
        file_handle: None,
    };
    policy.replace_mtproto(&alice, "primary", grant()).unwrap();
    assert_eq!(
        policy
            .authorized_mtproto(
                &alice,
                &req,
                bot,
                PolicyTime::Fixed(100),
                None,
                Some(&scope)
            )
            .unwrap_err()
            .kind(),
        ExecutorErrorKind::Denied
    );
    policy
        .replace_mtproto(&alice, "primary", grant().unscoped_updates(true))
        .unwrap();
    policy
        .authorized_mtproto(
            &alice,
            &req,
            bot,
            PolicyTime::Fixed(100),
            None,
            Some(&scope),
        )
        .unwrap();
}
