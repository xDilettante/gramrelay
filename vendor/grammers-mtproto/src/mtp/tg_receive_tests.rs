// Downstream receive-boundary regressions. Decrypted synthetic frames only.
use super::*;

fn receiver() -> Encrypted {
    Encrypted::build().finish([7; 256])
}
fn message(body: Vec<u8>) -> manual_tl::Message {
    manual_tl::Message {
        msg_id: 1,
        seq_no: 1,
        body,
    }
}
fn rpc(body: Vec<u8>) -> manual_tl::Message {
    let mut bytes = manual_tl::RpcResult::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    bytes.extend_from_slice(&42i64.to_le_bytes());
    bytes.extend(body);
    message(bytes)
}
fn rpc_error() -> Vec<u8> {
    tl::enums::RpcError::Error(tl::types::RpcError {
        error_code: 400,
        error_message: "X".into(),
    })
    .to_bytes()
}

#[test]
fn an_rpc_error_with_trailing_bytes_is_not_a_confirmed_refusal() {
    let mut state = receiver();
    let mut bytes = rpc_error();
    bytes.extend_from_slice(&[0; 4]);
    state.handle_rpc_result(rpc(bytes)).unwrap();
    assert!(
        !state
            .deserialization
            .iter()
            .any(|item| matches!(item, Deserialization::RpcError(_)))
    );
    assert!(
        state
            .deserialization
            .iter()
            .any(|item| matches!(item, Deserialization::Failure(_)))
    );
}

#[test]
fn a_complete_compressed_rpc_error_preserves_confirmed_refusal_semantics() {
    let mut state = receiver();
    state
        .handle_rpc_result(rpc(manual_tl::GzipPacked::new(&rpc_error()).to_bytes()))
        .unwrap();
    assert!(state.deserialization.iter().any(
        |item| matches!(item,Deserialization::RpcError(error) if error.error.error_code==400)
    ));
}

#[test]
fn trailing_bytes_after_the_gzip_tl_wrapper_are_rejected() {
    let mut state = receiver();
    let mut bytes = manual_tl::GzipPacked::new(&rpc_error()).to_bytes();
    bytes.extend_from_slice(&[0; 4]);
    state.handle_rpc_result(rpc(bytes)).unwrap();
    assert!(
        state
            .deserialization
            .iter()
            .any(|item| matches!(item, Deserialization::Failure(_)))
    );
}

#[test]
fn direct_pong_with_a_tail_is_rejected_before_exposing_a_result() {
    let mut state = receiver();
    let mut bytes = tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 42,
        ping_id: 1,
    })
    .to_bytes();
    bytes.extend_from_slice(&[0; 4]);
    assert!(state.process_message(message(bytes)).is_err());
    assert!(state.deserialization.is_empty());
}

#[test]
fn malformed_future_salts_never_mutate_the_live_salt_state() {
    let mut state = receiver();
    let original = state.salts.clone();
    let mut bytes = tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
        req_msg_id: 42,
        now: 100,
        salts: tl::RawVec(vec![tl::types::FutureSalt {
            valid_since: 1,
            valid_until: 200,
            salt: 123,
        }]),
    })
    .to_bytes();
    bytes.extend_from_slice(&[0; 4]);
    assert!(state.process_message(message(bytes)).is_err());
    assert_eq!(state.salts, original);
}

#[test]
fn response_cap_precedes_rpc_error_and_own_update_decoding() {
    for body in [
        rpc_error(),
        tl::enums::messages::AffectedMessages::Messages(tl::types::messages::AffectedMessages {
            pts: 1,
            pts_count: 1,
        })
        .to_bytes(),
    ] {
        let mut state = receiver();
        state.set_incoming_limits(8, 128, 32);
        state.handle_rpc_result(rpc(body)).unwrap();
        assert_eq!(state.deserialization.len(), 1);
        assert!(
            matches!(&state.deserialization[0],Deserialization::Failure(failure) if failure.error==DeserializeError::LimitExceeded)
        );
    }
}

#[test]
fn a_gzip_expansion_failure_exhausts_the_payload_budget() {
    let mut state = receiver();
    state.set_incoming_limits(32, 128, 32);
    let packed = manual_tl::GzipPacked::new(&[0; 4096]);
    assert!(matches!(
        packed.decompress_limited(32),
        Err(DeserializeError::LimitExceeded)
    ));
    state.handle_rpc_result(rpc(packed.to_bytes())).unwrap();
    assert!(
        matches!(&state.deserialization[0],Deserialization::Failure(failure) if failure.error==DeserializeError::LimitExceeded)
    );
    assert_eq!(
        state.expansion_used, 128,
        "a failed expansion cannot reset the budget for the next object"
    );
}

#[test]
fn gzip_budgets_are_shared_between_results_in_one_transport_payload() {
    let mut state = receiver();
    let length = rpc_error().len();
    state.set_incoming_limits(1024, length * 2 - 1, 32);
    for _ in 0..2 {
        state
            .handle_rpc_result(rpc(manual_tl::GzipPacked::new(&rpc_error()).to_bytes()))
            .unwrap();
    }
    assert_eq!(
        state
            .deserialization
            .iter()
            .filter(|item| matches!(item, Deserialization::RpcError(_)))
            .count(),
        1
    );
    assert!(state.deserialization.iter().any(|item|matches!(item,Deserialization::Failure(failure) if failure.error==DeserializeError::LimitExceeded)));
}

#[test]
fn nested_gzip_and_envelopes_have_a_fixed_depth_limit() {
    let mut state = receiver();
    state.set_incoming_limits(4096, 16384, 3);
    let mut body = rpc_error();
    for _ in 0..4 {
        body = manual_tl::GzipPacked::new(&body).to_bytes();
    }
    state.handle_rpc_result(rpc(body)).unwrap();
    assert!(
        matches!(&state.deserialization[0],Deserialization::Failure(failure) if failure.error==DeserializeError::LimitExceeded)
    );
    let mut body = rpc(rpc_error()).body;
    for _ in 0..4 {
        body = manual_tl::GzipPacked::new(&body).to_bytes();
    }
    assert!(matches!(
        state.process_message(message(body)),
        Err(DeserializeError::LimitExceeded)
    ));
    assert_eq!(state.nesting, 0);
}

#[test]
fn all_compressed_bytes_are_consumed_and_valid_members_may_split_one_tl_value() {
    let bytes = rpc_error();
    let mut packed = manual_tl::GzipPacked::new(&bytes[..8]);
    packed
        .packed_data
        .extend(manual_tl::GzipPacked::new(&bytes[8..]).packed_data);
    let mut state = receiver();
    state.handle_rpc_result(rpc(packed.to_bytes())).unwrap();
    assert!(matches!(
        &state.deserialization[0],
        Deserialization::RpcError(_)
    ));
    packed.packed_data.extend_from_slice(b"junk");
    let mut state = receiver();
    state.handle_rpc_result(rpc(packed.to_bytes())).unwrap();
    assert!(matches!(
        &state.deserialization[0],
        Deserialization::Failure(_)
    ));
}

#[test]
fn direct_control_replies_are_limited_before_typed_parsing() {
    let mut state = receiver();
    state.set_incoming_limits(8, 128, 32);
    let body = tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 42,
        ping_id: 1,
    })
    .to_bytes();
    assert!(matches!(
        state.process_message(message(body)),
        Err(DeserializeError::LimitExceeded)
    ));
    assert!(state.deserialization.is_empty());
}

#[test]
fn ignored_control_messages_still_require_complete_tl_values() {
    for constructor in [
        tl::types::DestroySessionOk::CONSTRUCTOR_ID,
        tl::types::MsgsStateInfo::CONSTRUCTOR_ID,
        tl::types::MsgsStateReq::CONSTRUCTOR_ID,
    ] {
        let mut state = receiver();
        assert!(
            state
                .process_message(message(constructor.to_le_bytes().to_vec()))
                .is_err()
        );
    }
    let mut state = receiver();
    let mut bytes = tl::types::DestroySessionOk::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&42i64.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    assert!(state.process_message(message(bytes)).is_err());
}

#[test]
fn noncanonical_byte_string_padding_never_becomes_a_confirmed_refusal() {
    let mut state = receiver();
    let mut bytes = rpc_error();
    bytes[10] = 1;
    state.handle_rpc_result(rpc(bytes)).unwrap();
    assert!(
        !state
            .deserialization
            .iter()
            .any(|item| matches!(item, Deserialization::RpcError(_)))
    );
    assert!(
        state
            .deserialization
            .iter()
            .any(|item| matches!(item, Deserialization::Failure(_)))
    );
}

#[test]
fn invalid_remote_lengths_return_errors_without_panicking() {
    for length in [-1, manual_tl::Message::MAX_BODY_SIZE as i32] {
        let mut bytes = 1i64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.extend_from_slice(&length.to_le_bytes());
        let result = std::panic::catch_unwind(|| manual_tl::Message::from_bytes(&bytes));
        assert!(matches!(result, Ok(Err(_))));
    }
    let mut bytes = manual_tl::MessageContainer::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&(-1i32).to_le_bytes());
    let mut state = receiver();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.process_message(message(bytes))
    }));
    assert!(matches!(result, Ok(Err(_))));
}

#[test]
fn unsupported_control_envelopes_fail_without_panicking() {
    for constructor in [
        manual_tl::MessageCopy::CONSTRUCTOR_ID,
        tl::types::FutureSalt::CONSTRUCTOR_ID,
    ] {
        let mut state = receiver();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.process_message(message(constructor.to_le_bytes().to_vec()))
        }));
        assert!(matches!(
            result,
            Ok(Err(DeserializeError::UnsupportedControl))
        ));
        assert_eq!(state.nesting, 0);
    }
}

#[test]
fn rejected_controls_preserve_the_acknowledgements_of_prior_messages_only() {
    for constructor in [
        tl::types::DestroySessionOk::CONSTRUCTOR_ID,
        manual_tl::MessageCopy::CONSTRUCTOR_ID,
        manual_tl::MessageContainer::CONSTRUCTOR_ID,
    ] {
        let mut state = receiver();
        state.pending_ack.push(99);
        assert!(
            state
                .process_message(message(constructor.to_le_bytes().to_vec()))
                .is_err()
        );
        assert_eq!(state.pending_ack, vec![99]);
    }
}

#[test]
fn a_rejected_container_rolls_back_the_effects_of_a_valid_prefix() {
    let mut state = receiver();
    let salts = state.salts.clone();
    state.pending_ack.push(99);
    let first = message(
        tl::enums::NewSession::Created(tl::types::NewSessionCreated {
            first_msg_id: 1,
            unique_id: 42,
            server_salt: 123,
        })
        .to_bytes(),
    );
    let second = message(
        tl::types::DestroySessionOk::CONSTRUCTOR_ID
            .to_le_bytes()
            .to_vec(),
    );
    let mut body = manual_tl::MessageContainer::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    body.extend_from_slice(&2i32.to_le_bytes());
    body.extend_from_slice(&first.to_bytes());
    body.extend_from_slice(&second.to_bytes());
    assert!(state.process_message(message(body)).is_err());
    assert_eq!(state.pending_ack, vec![99]);
    assert_eq!(state.salts, salts);
    assert!(state.deserialization.is_empty());
}
