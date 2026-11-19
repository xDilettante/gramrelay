// Downstream receive-boundary regressions. Decrypted synthetic frames only.
use super::*;

fn receiver() -> Encrypted {
    Encrypted::build().finish([7; 256])
}

fn record_sent_header(state: &mut Encrypted, id: i64, sequence: i32) {
    state.sent_history.regular.push_back(SentHeader {
        id: MsgId(id),
        sequence,
        sent_at: Instant::now(),
        rejected: false,
    });
}

fn bad_sequence(id: MsgId, sequence: i32) -> manual_tl::Message {
    message(
        tl::enums::BadMsgNotification::Notification(tl::types::BadMsgNotification {
            bad_msg_id: id.0,
            bad_msg_seqno: sequence,
            error_code: 32,
        })
        .to_bytes(),
    )
}

fn new_session(first_id: i64, notification_id: i64, unique_id: i64) -> manual_tl::Message {
    let mut session = message(
        tl::enums::NewSession::Created(tl::types::NewSessionCreated {
            first_msg_id: first_id,
            unique_id,
            server_salt: unique_id + 100,
        })
        .to_bytes(),
    );
    session.msg_id = notification_id;
    session
}

#[test]
fn only_completed_transport_writes_record_container_and_message_headers() {
    let mut connection = crate::connection::State::new(crate::transport::Full::new(), receiver());
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    let first = connection.buffer_request(&ping).unwrap();
    let second = connection.buffer_request(&ping).unwrap();
    let container = connection.pack_outgoing_buffer().unwrap();
    let length = connection.io_buffers().1.len();
    assert!(connection.mtp.sent_history.regular.is_empty());
    assert!(!connection.handle_net_write(length - 1));
    assert!(connection.mtp.sent_history.regular.is_empty());
    assert!(connection.handle_net_write(1));
    let headers = connection
        .mtp
        .sent_history
        .regular
        .iter()
        .map(|header| (header.id, header.sequence))
        .collect::<Vec<_>>();
    assert_eq!(headers, [(first, 1), (second, 3), (container, 4)]);
}

#[test]
fn idle_transport_keeps_receipts_due_until_the_packet_finishes_writing() {
    let mut connection = crate::connection::State::new(crate::transport::Full::new(), receiver());
    let body = tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 4,
        ping_id: 1,
    })
    .to_bytes();
    connection
        .mtp
        .process_message(message(body.clone()))
        .unwrap();
    let receipt = connection.pack_outgoing_buffer().unwrap();
    let length = connection.io_buffers().1.len();
    assert_eq!(connection.mtp.acknowledgements.pending, [1]);
    assert!(!connection.handle_net_write(0));
    assert!(!connection.handle_net_write(length - 1));
    assert_eq!(connection.mtp.acknowledgements.pending, [1]);
    let mut next = message(body);
    next.msg_id = 3;
    connection.mtp.process_message(next).unwrap();
    assert_eq!(connection.mtp.acknowledgements.pending, [1, 3]);
    assert!(connection.handle_net_write(1));
    assert_eq!(connection.mtp.acknowledgements.pending, [3]);
    assert_eq!(
        connection.mtp.sent_history.standalone_receipts[0].id,
        receipt
    );
    assert_eq!(
        connection.mtp.sent_history.standalone_receipts[0].sequence,
        0
    );
    connection.pack_outgoing_buffer().unwrap();
    let length = connection.io_buffers().1.len();
    assert!(connection.handle_net_write(length));
    assert!(connection.mtp.acknowledgements.pending.is_empty());
    assert!(connection.pack_outgoing_buffer().is_none());
}

#[test]
fn receipt_queue_deduplicates_and_sends_bounded_batches() {
    let mut state = receiver();
    let limit = ACK_QUEUE_LIMIT as i64;
    for id in 1..=limit {
        state.acknowledgements.enqueue(id).unwrap();
    }
    state.acknowledgements.enqueue(1).unwrap();
    assert_eq!(state.acknowledgements.pending.len(), ACK_QUEUE_LIMIT);
    assert_eq!(
        state.acknowledgements.enqueue(limit + 1),
        Err(DeserializeError::LimitExceeded)
    );
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    state.buffer_acknowledgements(&mut buffer).unwrap();
    let frame = manual_tl::Message::deserialize(&mut Cursor::from_slice(buffer.as_ref())).unwrap();
    let tl::enums::MsgsAck::Ack(ack) = decode_exact::<tl::enums::MsgsAck>(&frame.body).unwrap();
    assert_eq!(
        ack.msg_ids,
        (1..=ACK_BATCH_LIMIT as i64).collect::<Vec<_>>()
    );
    assert_eq!(frame.seq_no, 0);
    let first_receipt = state.finalize(&mut buffer).unwrap();

    buffer.clear();
    let replacement = state.finalize(&mut buffer).unwrap();
    assert_ne!(replacement, first_receipt);
    assert_eq!(state.acknowledgements.pending.len(), ACK_QUEUE_LIMIT);
    state.on_packet_sent();
    assert_eq!(state.acknowledgements.pending.len(), ACK_BATCH_LIMIT);
    assert_eq!(
        state.acknowledgements.pending.front(),
        Some(&(ACK_BATCH_LIMIT as i64 + 1))
    );
    buffer.clear();
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    assert!(state.acknowledgements.pending.is_empty());
    assert!(state.acknowledgements.pending_ids.is_empty());
    assert_eq!(
        state.acknowledgements.recently_written.len(),
        ACK_BATCH_LIMIT
    );
    assert_eq!(state.acknowledgements.written_ids.len(), ACK_BATCH_LIMIT);
    assert!(!state.acknowledgements.written_ids.contains(&1));
}

#[test]
fn detailed_info_acknowledges_only_a_recently_received_body() {
    let mut state = receiver();
    let mut info = message(
        tl::enums::MsgDetailedInfo::MsgNewDetailedInfo(tl::types::MsgNewDetailedInfo {
            answer_msg_id: 5,
            bytes: 20,
            status: 0,
        })
        .to_bytes(),
    );
    info.seq_no = 0;
    let info_body = info.to_bytes();
    state.process_message(info).unwrap();
    assert!(state.acknowledgements.pending.is_empty());

    let mut received = message(
        tl::enums::Pong::Pong(tl::types::Pong {
            msg_id: 4,
            ping_id: 1,
        })
        .to_bytes(),
    );
    received.msg_id = 5;
    state.process_message(received).unwrap();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    assert!(state.acknowledgements.pending.is_empty());
    let info = manual_tl::Message::deserialize(&mut Cursor::from_slice(&info_body)).unwrap();
    state.process_message(info).unwrap();
    assert_eq!(state.acknowledgements.pending, [5]);
}

#[test]
fn a_full_size_request_gets_room_during_continuous_receipts() {
    let mut state = Encrypted::build()
        .compression_threshold(None)
        .finish([7; 256]);
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    let body = vec![0; manual_tl::Message::MAX_BODY_SIZE];
    for id in 1..=8 {
        state.acknowledgements.enqueue(id).unwrap();
    }
    assert!(state.push(&mut buffer, &body).is_none());
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    buffer.clear();
    for id in 9..=16 {
        state.acknowledgements.enqueue(id).unwrap();
    }
    assert!(state.push(&mut buffer, &body).is_some());
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    assert_eq!(state.acknowledgements.pending.len(), 8);
    buffer.clear();
    assert!(state.push(&mut buffer, &body).is_none());
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    assert!(state.acknowledgements.pending.is_empty());
}

#[test]
fn acknowledgement_only_output_preserves_regular_correlation_under_pressure() {
    let mut state = receiver();
    let salt_request = MsgId(4);
    state.salt_request_msg_id = Some(salt_request);
    for ordinal in 1..=SENT_HISTORY_LIMIT as i64 {
        record_sent_header(&mut state, ordinal * 4, 1);
    }
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    for id in 1..=(2 * STANDALONE_RECEIPT_HISTORY_LIMIT) as i64 {
        state.acknowledgements.enqueue(id).unwrap();
        state.finalize(&mut buffer).unwrap();
        state.on_packet_sent();
        buffer.clear();
    }
    assert!(state.acknowledgements.pending.is_empty());
    assert_eq!(state.sent_history.regular.len(), SENT_HISTORY_LIMIT);
    assert_eq!(
        state.sent_history.standalone_receipts.len(),
        STANDALONE_RECEIPT_HISTORY_LIMIT
    );
    assert!(matches!(
        state.sent_history.find(salt_request),
        Some(SentHeaderLocation::Regular(0))
    ));
    assert!(matches!(
        state.sent_history.find(MsgId(8)),
        Some(SentHeaderLocation::Regular(1))
    ));
    state
        .process_message(message(
            tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
                req_msg_id: salt_request.0,
                now: 100,
                salts: tl::RawVec(Vec::new()),
            })
            .to_bytes(),
        ))
        .unwrap();
    assert!(state.salt_request_msg_id.is_none());
}

#[test]
fn idle_receipts_advance_the_salt_schedule_before_serialization() {
    let mut state = Encrypted::build().first_salt(1).finish([7; 256]);
    state.salts = vec![
        tl::types::FutureSalt {
            valid_since: 0,
            valid_until: 100,
            salt: 1,
        },
        tl::types::FutureSalt {
            valid_since: 90,
            valid_until: 300,
            salt: 2,
        },
    ];
    state.start_salt_time = Some((120, Instant::now()));
    state.acknowledgements.enqueue(3).unwrap();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    assert!(state.finalize(&mut buffer).is_some());
    assert_eq!(state.current_salt, 2);
    assert_eq!(state.acknowledgements.in_packet, 1);
}

#[test]
fn the_salt_request_leaves_alone_then_receipts_can_flush_while_it_waits() {
    let mut state = Encrypted::build().first_salt(123).finish([7; 256]);
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    state.acknowledgements.enqueue(3).unwrap();
    let expected_salt_request = state.finalize(&mut buffer).unwrap();
    assert_eq!(state.salt_request_msg_id, Some(expected_salt_request));
    assert_eq!(state.acknowledgements.in_packet, 0);
    state.on_packet_sent();

    buffer.clear();
    assert!(state.finalize(&mut buffer).is_some());
    assert_eq!(state.acknowledgements.in_packet, 1);
    state.on_packet_sent();
    assert!(state.acknowledgements.pending.is_empty());
    assert_eq!(state.salt_request_msg_id, Some(expected_salt_request));
}

#[test]
fn incoming_recreation_replaces_an_obsolete_unpacked_salt_request() {
    let mut state = Encrypted::build().first_salt(123).finish([7; 256]);
    state.session_established = true;
    record_sent_header(&mut state, 4, 1);
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    assert!(state.push(&mut buffer, &ping).is_none());
    let obsolete = state.salt_request_msg_id.unwrap();
    state.process_message(new_session(4, 5, 1)).unwrap();
    assert!(state.salt_request_msg_id.is_none());

    let replacement = state.finalize(&mut buffer).unwrap();
    assert_ne!(replacement, obsolete);
    assert_eq!(state.salt_request_msg_id, Some(replacement));
    assert_eq!(state.acknowledgements.in_packet, 0);
    state.on_packet_sent();
    assert!(state.sent_history.find(obsolete).is_none());
    assert!(state.sent_history.find(replacement).is_some());
    assert_eq!(state.acknowledgements.pending, [5]);
}

#[test]
fn overflowing_a_frame_rolls_back_its_receipts_only() {
    let mut state = receiver();
    for id in 1..ACK_QUEUE_LIMIT as i64 {
        state.acknowledgements.enqueue(id).unwrap();
    }
    let pong = tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 4,
        ping_id: 1,
    })
    .to_bytes();
    let mut first = message(pong.clone());
    first.msg_id = ACK_QUEUE_LIMIT as i64;
    let mut second = message(pong);
    second.msg_id = first.msg_id + 1;
    let mut frame = container(&[&first, &second]);
    frame.seq_no = 0;
    assert_eq!(
        state.process_message(frame),
        Err(DeserializeError::LimitExceeded)
    );
    assert_eq!(state.acknowledgements.pending.len(), ACK_QUEUE_LIMIT - 1);
    assert!(!state.acknowledgements.pending_ids.contains(&first.msg_id));
    assert!(state.deserialization.is_empty());
    state.process_message(first).unwrap();
    assert_eq!(state.acknowledgements.pending.len(), ACK_QUEUE_LIMIT);
}

#[test]
fn a_sequence_notice_matches_a_recent_written_header_only_once() {
    let mut state = receiver();
    let id = MsgId(4);
    state.process_message(bad_sequence(id, 1)).unwrap();
    assert_eq!(state.sequence, 0);
    record_sent_header(&mut state, id.0, 1);
    state.process_message(bad_sequence(id, 3)).unwrap();
    assert_eq!(state.sequence, 0);
    state.process_message(bad_sequence(id, 1)).unwrap();
    state.process_message(bad_sequence(id, 1)).unwrap();
    assert_eq!(state.sequence, 64);
    assert!(
        matches!(&state.deserialization[..], [Deserialization::BadMessage(item)] if item.msg_id == id)
    );
}

#[test]
fn session_recreation_reports_a_gap_only_for_a_fresh_matching_notification() {
    let mut state = receiver();
    state.process_message(new_session(4, 5, 1)).unwrap();
    assert_eq!(state.current_salt, 0);
    record_sent_header(&mut state, 4, 1);
    state.process_message(new_session(4, 5, 1)).unwrap();
    assert_eq!(state.current_salt, 101);
    assert!(state.deserialization.is_empty());
    state.process_message(new_session(4, 9, 2)).unwrap();
    state.process_message(new_session(4, 13, 2)).unwrap();
    state.process_message(new_session(4, 5, 1)).unwrap();
    assert_eq!(state.current_salt, 102);
    assert!(matches!(
        &state.deserialization[..],
        [Deserialization::SessionRecreated]
    ));
}

#[test]
fn a_completed_reply_establishes_the_session_without_a_creation_notice() {
    let mut unconfirmed = receiver();
    record_sent_header(&mut unconfirmed, 42, 0);
    unconfirmed.process_message(rpc(rpc_error())).unwrap();
    assert!(!unconfirmed.session_established);

    let mut state = receiver();
    record_sent_header(&mut state, 42, 1);
    state.process_message(rpc(rpc_error())).unwrap();
    assert!(state.session_established);
    state.process_message(new_session(42, 5, 1)).unwrap();
    assert!(matches!(
        state.deserialization.last(),
        Some(Deserialization::SessionRecreated)
    ));
}

#[test]
fn an_aged_internal_rejection_releases_refresh_without_correcting_state() {
    let mut state = receiver();
    let id = MsgId(4);
    record_sent_header(&mut state, id.0, 1);
    state.sent_history.regular[0].sent_at -= SENT_HISTORY_AGE + Duration::from_secs(1);
    state.salt_request_msg_id = Some(id);
    state.process_message(bad_sequence(id, 1)).unwrap();
    assert_eq!(state.sequence, 0);
    assert!(state.salt_request_msg_id.is_none());
    assert!(state.deserialization.is_empty());
}

#[test]
fn a_discarded_finalized_packet_does_not_enter_sent_history() {
    let mut state = receiver();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    let discarded = state.push(&mut buffer, &ping).unwrap();
    state.finalize(&mut buffer).unwrap();
    buffer.clear();
    let sent = state.push(&mut buffer, &ping).unwrap();
    state.finalize(&mut buffer).unwrap();
    state.on_packet_sent();
    assert!(state.sent_history.find(discarded).is_none());
    assert!(state.sent_history.find(sent).is_some());
}

#[test]
fn clock_correction_handles_the_entire_wire_timestamp_without_wrapping() {
    let far_timestamp = (u64::from(u32::MAX) << 32 | 1) as i64;
    let correct_at = |msg, seconds| clock_correction(msg, Duration::from_secs(seconds), 0);
    assert_eq!(correct_at(far_timestamp, u64::from(u32::MAX) - 8), Ok(8));
    assert_eq!(correct_at(2i64 << 32, 12), Ok(-10));
    assert_eq!(
        correct_at(far_timestamp, 1),
        Err(DeserializeError::InvalidEncoding)
    );
    assert_eq!(
        correct_at(1, u64::MAX),
        Err(DeserializeError::InvalidEncoding)
    );
}

#[test]
fn impossible_same_session_clock_correction_is_rejected() {
    let server = (123i64 << 32) | 1;
    let local = Duration::from_secs(124);
    assert_eq!(clock_correction(server, local, 130i64 << 32), Ok(-1));
    assert_eq!(
        clock_correction(server, local, 153i64 << 32),
        Err(DeserializeError::InvalidEncoding)
    );
}

#[test]
fn message_identifiers_are_aligned_monotonic_and_have_a_fraction() {
    let whole_second = Duration::from_secs(123);
    assert_eq!(next_msg_id(whole_second, 0, 0), Ok((123 << 32) | 4));
    let signed_end = i64::MAX & !3;
    assert_eq!(next_msg_id(whole_second, 0, signed_end), Ok(i64::MIN + 4));
    assert_eq!(
        next_msg_id(whole_second, 0, -4),
        Err(SerializeError::MessageIdsExhausted)
    );
    assert_eq!(
        next_msg_id(whole_second, -124, 0),
        Err(SerializeError::InvalidClock)
    );
}

#[test]
fn sequence_corrections_preserve_the_even_nonnegative_counter() {
    let mut state = receiver();
    assert_eq!(
        state.correct_sequence(-16),
        Err(DeserializeError::InvalidEncoding)
    );
    assert_eq!(state.sequence, 0);
    state.correct_sequence(64).unwrap();
    state.correct_sequence(-16).unwrap();
    assert_eq!(state.sequence, 48);
    state.sequence = i32::MAX - 65;
    assert_eq!(
        state.correct_sequence(64),
        Err(DeserializeError::InvalidEncoding)
    );
    assert_eq!(state.sequence, i32::MAX - 65);
}

#[test]
fn header_exhaustion_leaves_buffer_unsendable_and_error_sticky() {
    let mut state = receiver();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    assert!(state.push(&mut buffer, &ping).is_some());
    let length = buffer.len();
    state.sequence = i32::MAX - 1;
    let last = state.last_msg_id;
    assert!(state.push(&mut buffer, &ping).is_none());
    assert_eq!(buffer.len(), length);
    assert_eq!(state.last_msg_id, last);
    assert_eq!(
        state.serialize_error(),
        Some(SerializeError::SequenceExhausted)
    );
    assert!(state.finalize(&mut buffer).is_none());
    assert!(state.push(&mut buffer, &ping).is_none());
}

#[test]
fn salt_activation_uses_wide_time_arithmetic() {
    assert_eq!(
        salt_time(i32::MAX - 10, Duration::from_secs(60)),
        i128::from(i32::MAX) + 50
    );
    assert_eq!(
        salt_time(10, Duration::from_secs(u64::MAX)),
        i128::from(u64::MAX) + 10
    );
}

#[test]
fn plaintext_session_must_match_before_decoding_the_body() {
    let mut state = receiver();
    let header = |id: i64| [0i64.to_le_bytes(), id.to_le_bytes()].concat();
    let session = state.session_id;
    state.expansion_used = 19;
    assert_eq!(
        state.process_plaintext(&header(session)),
        Err(DeserializeError::MessageBufferTooSmall)
    );
    assert_eq!(state.expansion_used, 0);

    state.expansion_used = 31;
    let error = state.process_plaintext(&header(session ^ 1)).unwrap_err();
    assert_eq!(error, DeserializeError::UnexpectedSession);
    assert_eq!(error.to_string(), "unexpected protocol session");
    assert_eq!(state.expansion_used, 31);
    assert!(state.deserialization.is_empty());
}

fn message(body: Vec<u8>) -> manual_tl::Message {
    manual_tl::Message {
        msg_id: 1,
        seq_no: 1,
        body,
    }
}
fn container(children: &[&manual_tl::Message]) -> manual_tl::Message {
    let mut body = manual_tl::MessageContainer::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    body.extend_from_slice(&(children.len() as i32).to_le_bytes());
    for child in children {
        body.extend_from_slice(&child.to_bytes());
    }
    message(body)
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
fn only_a_matching_internal_reply_replaces_the_salt_schedule() {
    let mut state = receiver();
    let original = state.salts.clone();
    let reply = |req_msg_id, salts| {
        message(
            tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
                req_msg_id,
                now: 100,
                salts: tl::RawVec(salts),
            })
            .to_bytes(),
        )
    };
    let salt = |valid_since, valid_until, salt| tl::types::FutureSalt {
        valid_since,
        valid_until,
        salt,
    };
    state.salt_request_msg_id = Some(MsgId(42));
    state
        .process_message(reply(41, vec![salt(10, 110, 1)]))
        .unwrap();
    assert_eq!(state.salts, original);
    assert_eq!(state.salt_request_msg_id, Some(MsgId(42)));
    assert!(matches!(&state.deserialization[..],
        [Deserialization::RpcResult(result)] if result.msg_id == MsgId(41)));
    state
        .process_message(reply(42, vec![salt(10, 110, 2)]))
        .unwrap();
    assert_eq!(state.salts, original);
    assert_eq!(state.salt_request_msg_id, Some(MsgId(42)));
    record_sent_header(&mut state, 42, 1);
    assert_eq!(
        state.process_message(reply(42, vec![salt(100, 100, 2)])),
        Err(DeserializeError::InvalidEncoding)
    );
    assert_eq!(state.salts, original);
    assert_eq!(state.salt_request_msg_id, Some(MsgId(42)));
    state
        .process_message(reply(
            42,
            vec![
                salt(122, 200, 5),
                salt(20, 120, 3),
                salt(10, 110, 4),
                salt(1, 80, 9),
            ],
        ))
        .unwrap();
    assert_eq!(
        state.salts,
        vec![salt(122, 200, 5), salt(20, 120, 3), salt(10, 110, 4)]
    );
    assert!(state.salt_request_msg_id.is_none());
    state.advance_salts(121);
    assert_eq!(
        state.get_current_salt(),
        3,
        "the future salt is not active yet"
    );
    state.advance_salts(122);
    assert_eq!(state.get_current_salt(), 5);
}

#[test]
fn internal_salt_refresh_never_bundles_with_existing_requests() {
    let mut state = receiver();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    assert!(state.push(&mut buffer, &ping).is_some());
    state.current_salt = 123;
    assert!(state.push(&mut buffer, &ping).is_some());
    assert!(state.salt_request_msg_id.is_none());
}

#[test]
fn an_older_active_interval_can_outlive_a_newer_salt() {
    let mut state = receiver();
    state.salts = vec![tl::types::FutureSalt {
        valid_since: 10,
        valid_until: 350,
        salt: 1,
    }];
    state.current_salt = 1;
    state.salt_request_msg_id = Some(MsgId(42));
    record_sent_header(&mut state, 42, 1);
    state
        .process_message(message(
            tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
                req_msg_id: 42,
                now: 175,
                salts: tl::RawVec(vec![tl::types::FutureSalt {
                    valid_since: 100,
                    valid_until: 200,
                    salt: 2,
                }]),
            })
            .to_bytes(),
        ))
        .unwrap();
    assert_eq!(state.get_current_salt(), 2);
    state.advance_salts(200);
    assert_eq!(state.get_current_salt(), 1);
    assert_eq!(state.salts.len(), 1);
}

#[test]
fn empty_internal_salt_reply_retains_current_salt_and_allows_requests() {
    let mut state = Encrypted::build().first_salt(123).finish([7; 256]);
    state.salt_request_msg_id = Some(MsgId(42));
    record_sent_header(&mut state, 42, 1);
    state.sent_history.regular[0].sent_at -= SENT_HISTORY_AGE + Duration::from_secs(1);
    state
        .process_message(message(
            tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
                req_msg_id: 42,
                now: 100,
                salts: tl::RawVec(Vec::new()),
            })
            .to_bytes(),
        ))
        .unwrap();
    let mut buffer = DequeBuffer::with_capacity(512, 128);
    assert!(
        state
            .push(&mut buffer, &tl::functions::Ping { ping_id: 1 }.to_bytes())
            .is_some()
    );
    assert_eq!(state.get_current_salt(), 123);
    assert!(state.salt_request_msg_id.is_none());
}

#[test]
fn recreation_preserves_a_refresh_that_belongs_to_the_new_generation() {
    let session = new_session(4, 5, 1);
    let reply = message(
        tl::enums::FutureSalts::Salts(tl::types::FutureSalts {
            req_msg_id: 4,
            now: 100,
            salts: tl::RawVec(vec![tl::types::FutureSalt {
                valid_since: 70,
                valid_until: 200,
                salt: 123,
            }]),
        })
        .to_bytes(),
    );
    for children in [[&session, &reply], [&reply, &session]] {
        let mut state = receiver();
        state.session_established = true;
        record_sent_header(&mut state, 4, 1);
        state.salt_request_msg_id = Some(MsgId(4));
        state.process_message(container(&children)).unwrap();
        assert_eq!(state.current_salt, 123);
        assert!(state.salt_request_msg_id.is_none());
        assert!(matches!(
            &state.deserialization[..],
            [Deserialization::SessionRecreated]
        ));
    }
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
        state.acknowledgements.enqueue(99).unwrap();
        assert!(
            state
                .process_message(message(constructor.to_le_bytes().to_vec()))
                .is_err()
        );
        assert_eq!(state.acknowledgements.pending, [99]);
        assert_eq!(state.acknowledgements.pending_ids, HashSet::from([99]));
    }
}

#[test]
fn a_rejected_container_rolls_back_the_effects_of_a_valid_prefix() {
    let mut state = receiver();
    let salts = state.salts.clone();
    let current = state.get_current_salt();
    state.acknowledgements.enqueue(99).unwrap();
    record_sent_header(&mut state, 4, 1);
    let first = new_session(4, 5, 23);
    let rejection = bad_sequence(MsgId(4), 1);
    let second = message(
        tl::types::DestroySessionOk::CONSTRUCTOR_ID
            .to_le_bytes()
            .to_vec(),
    );
    assert!(
        state
            .process_message(container(&[&first, &rejection, &second]))
            .is_err()
    );
    assert_eq!(state.acknowledgements.pending, [99]);
    assert_eq!(state.salts, salts);
    assert_eq!(state.get_current_salt(), current);
    assert_eq!(state.sequence, 0);
    assert!(state.deserialization.is_empty());
    state.process_message(first).unwrap();
    state.process_message(rejection).unwrap();
    assert_eq!(state.get_current_salt(), 123);
    assert_eq!(state.sequence, 64);
}
