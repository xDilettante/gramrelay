use super::*;
use tl::Identifiable;
fn baseline() -> UpdatesState {
    UpdatesState {
        pts: 10,
        qts: 0,
        seq: 0,
        date: 100,
        channels: vec![grammers_session::types::ChannelState { id: 9, pts: 5 }],
    }
}
fn ready() -> Machine {
    let mut machine = Machine::load(baseline()).ok().unwrap();
    machine
        .difference(tl::types::updates::DifferenceEmpty { date: 100, seq: 0 }.into())
        .ok()
        .unwrap();
    machine
        .channel(
            9,
            tl::types::updates::ChannelDifferenceEmpty {
                r#final: true,
                pts: 5,
                timeout: None,
            }
            .into(),
        )
        .ok()
        .unwrap();
    machine
}
fn raw(update: tl::enums::Update) -> UpdatesLike {
    UpdatesLike::Updates(tl::types::UpdateShort { update, date: 100 }.into())
}
#[test]
fn malformed_marker_and_semantic_overflow_become_explicit_gaps() {
    let mut machine = ready();
    assert!(matches!(
        machine.socket(UpdatesLike::MalformedUpdates),
        Err((GapScope::Account, GapReason::Malformed))
    ));
    let mut machine = ready();
    let input = raw(tl::types::UpdateDeleteMessages {
        messages: vec![1],
        pts: 11,
        pts_count: i32::MIN,
    }
    .into());
    assert!(matches!(
        machine.socket(input),
        Err((GapScope::Account, GapReason::Malformed))
    ));
}
#[test]
fn channel_hints_preserve_known_pts_and_missing_baseline_is_explicit() {
    let mut machine = ready();
    machine
        .socket(raw(tl::types::UpdateChannelTooLong {
            channel_id: 9,
            pts: Some(999),
        }
        .into()))
        .ok()
        .unwrap();
    assert_eq!(machine.state().channels[0].pts, 5);
    assert!(matches!(machine.work(),Work::Channel(request) if request.pts==5));
    let mut machine = ready();
    assert!(matches!(
        machine.socket(raw(tl::types::UpdateChannelTooLong {
            channel_id: 10,
            pts: Some(999)
        }
        .into())),
        Err((GapScope::Channel(10), GapReason::MissingBaseline))
    ));
    assert_eq!(machine.state().channels.len(), 1);
}
#[test]
fn too_long_is_never_applied_and_backlog_is_bounded_by_actual_encoded_data() {
    let mut machine = ready();
    assert!(matches!(
        machine.difference(tl::types::updates::DifferenceTooLong { pts: 999 }.into()),
        Err((GapScope::Account, GapReason::AccountTooLong))
    ));
    assert!(machine.state() == baseline());
    let mut machine = ready();
    let make = || {
        raw(tl::types::UpdateDeleteMessages {
            messages: vec![1; 100000],
            pts: 12,
            pts_count: 1,
        }
        .into())
    };
    machine.socket(make()).ok().unwrap();
    assert_eq!(machine.boxes.buffered_updates(), 1);
    machine.socket(make()).ok().unwrap();
    assert!(matches!(
        machine.socket(make()),
        Err((GapScope::Account, GapReason::BacklogLimit))
    ));
}

#[test]
fn short_message_adaptation_cannot_expand_a_retained_backlog_past_the_byte_cap() {
    let mut machine = ready();
    let text = "x".repeat(MAX_BATCH_BYTES - 36);
    let mut bytes = tl::types::UpdateShortMessage::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&1i32.to_le_bytes());
    bytes.extend_from_slice(&42i64.to_le_bytes());
    bytes.extend_from_slice(&text.to_bytes());
    bytes.extend_from_slice(&12i32.to_le_bytes());
    bytes.extend_from_slice(&1i32.to_le_bytes());
    bytes.extend_from_slice(&100i32.to_le_bytes());
    assert_eq!(bytes.len(), MAX_BATCH_BYTES);
    let raw = UpdatesLike::Updates(tl::enums::Updates::from_bytes(&bytes).unwrap());
    assert!(matches!(
        machine.socket(raw),
        Err((GapScope::Account, GapReason::BacklogLimit))
    ));
}

#[test]
fn explicit_zero_baselines_always_start_difference_without_guessing_first_event_pts() {
    for qts in [0, 1] {
        let mut machine = Machine::load(UpdatesState {
            pts: 0,
            qts,
            date: 100,
            seq: 0,
            channels: vec![],
        })
        .unwrap();
        assert!(
            matches!(machine.work(),Work::Difference(request) if request.pts==0 && request.qts==qts)
        );
    }
}
#[test]
fn negative_counts_and_unknown_channel_pts_never_enter_sdk_sequencing() {
    for pts in [0, 11] {
        let mut machine = ready();
        let raw = UpdatesLike::Updates(
            tl::types::UpdateShort {
                update: tl::types::UpdateDeleteMessages {
                    messages: vec![1],
                    pts,
                    pts_count: -1,
                }
                .into(),
                date: 100,
            }
            .into(),
        );
        assert!(matches!(
            machine.socket(raw),
            Err((GapScope::Account, GapReason::Malformed))
        ));
    }
    let mut machine = ready();
    let raw = UpdatesLike::Updates(
        tl::types::UpdateShort {
            update: tl::types::UpdateDeleteChannelMessages {
                channel_id: 10,
                messages: vec![1],
                pts: 12,
                pts_count: 1,
            }
            .into(),
            date: 100,
        }
        .into(),
    );
    assert!(matches!(
        machine.socket(raw),
        Err((GapScope::Channel(10), GapReason::MissingBaseline))
    ));
}

#[test]
fn sdk_short_sent_channel_envelope_is_normalized_without_a_false_malformed_gap() {
    for peer in [
        tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
            channel_id: 9,
            access_hash: 123,
        }),
        tl::enums::InputPeer::ChannelFromMessage(Box::new(
            tl::types::InputPeerChannelFromMessage {
                peer: tl::enums::InputPeer::PeerSelf,
                msg_id: 1,
                channel_id: 9,
            },
        )),
    ] {
        let mut machine = ready();
        let mut body = 0u32.to_le_bytes().to_vec();
        body.extend_from_slice(&peer.to_bytes());
        body.extend_from_slice(&"private channel body".to_owned().to_bytes());
        body.extend_from_slice(&7i64.to_le_bytes());
        let request = tl::functions::messages::SendMessage::from_bytes(&body).unwrap();
        let update = tl::types::UpdateShortSentMessage {
            out: true,
            id: 1,
            pts: 6,
            pts_count: 1,
            date: 100,
            media: None,
            entities: None,
            ttl_period: None,
        };
        let packet = machine
            .socket(UpdatesLike::ShortSentMessage { request, update })
            .ok()
            .unwrap();
        assert!(
            matches!(packet.events.as_slice(),[tl::enums::Update::NewChannelMessage(message)] if message.pts==6)
        );
        assert_eq!(machine.state().pts, 10);
        assert_eq!(machine.state().channels[0].pts, 6);
    }
}

#[test]
fn sdk_trace_cannot_format_private_update_content_inside_the_engine() {
    use std::io::Write;
    #[derive(Clone)]
    struct Writer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let writer = Writer(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("visible control event");
        let mut machine = ready();
        let packet = machine
            .socket(raw(tl::types::UpdateUserName {
                user_id: 42,
                first_name: "private first name".into(),
                last_name: "private last name".into(),
                usernames: vec![],
            }
            .into()))
            .ok()
            .unwrap();
        assert_eq!(packet.events.len(), 1);
    });
    let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(output.contains("visible control event"));
    assert!(!output.contains("private"));
    assert!(!output.contains("processing updates"));
}

#[test]
fn duplicate_large_counts_use_mathematical_comparisons_without_false_gaps() {
    let mut machine = ready();
    let event: tl::enums::Update = tl::types::UpdateDeleteMessages {
        messages: vec![1],
        pts: 1_500_000_010,
        pts_count: 1_500_000_000,
    }
    .into();
    let input = UpdatesLike::Updates(
        tl::types::Updates {
            updates: vec![event.clone(), event],
            users: vec![],
            chats: vec![],
            date: 100,
            seq: 0,
        }
        .into(),
    );
    let packet = machine
        .socket(input)
        .expect("counter comparisons must not wrap");
    assert_eq!(packet.events.len(), 1);
    assert_eq!(machine.boxes.buffered_updates(), 0);
    assert_eq!(machine.state().pts, 1_500_000_010);
}
