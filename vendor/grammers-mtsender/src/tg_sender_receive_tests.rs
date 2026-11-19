// Local decoder/diagnostic regressions. Synthetic credentials and localhost only.
use super::*;
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn recovery_deadline_wakes_an_idle_sender_and_completes_only_the_sent_request() {
    struct TimedRecovery {
        plain: mtp::Plain,
        request: Option<MsgId>,
        deadline: Option<std::time::Instant>,
    }
    impl mtp::Mtp for TimedRecovery {
        fn push(
            &mut self,
            buffer: &mut grammers_mtproto::DequeBuffer<u8>,
            body: &[u8],
        ) -> Option<MsgId> {
            let id = self.plain.push(buffer, body)?;
            self.request = Some(id);
            Some(id)
        }
        fn finalize(&mut self, buffer: &mut grammers_mtproto::DequeBuffer<u8>) -> Option<MsgId> {
            self.plain.finalize(buffer)
        }
        fn on_packet_sent(&mut self) {
            self.deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(20));
        }
        fn deserialize(
            &mut self,
            body: &mut [u8],
        ) -> Result<Vec<mtp::Deserialization>, mtp::DeserializeError> {
            self.plain.deserialize(body)
        }
        fn poll_recovery(
            &mut self,
            now: std::time::Instant,
            results: &mut Vec<mtp::Deserialization>,
        ) -> Option<std::time::Instant> {
            if self.deadline.is_some_and(|deadline| deadline <= now) {
                self.deadline = None;
                results.push(mtp::Deserialization::ResponseUnavailable {
                    target: mtp::RecoveryTarget::Request(self.request.unwrap()),
                });
            }
            self.deadline
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut sender = Sender::connect(
        grammers_mtproto::transport::Full::new(),
        TimedRecovery {
            plain: mtp::Plain::new(),
            request: None,
            deadline: None,
        },
        ServerAddr::Tcp {
            address: listener.local_addr().unwrap(),
        },
    )
    .await
    .unwrap();
    let (_server, _) = listener.accept().await.unwrap();
    let (reply, waiting) = oneshot::channel();
    sender.enqueue_body(tl::functions::Ping { ping_id: 1 }.to_bytes(), reply);

    let gap = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let updates = sender.step().await.unwrap();
            if !updates.is_empty() {
                break updates;
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(gap.as_slice(), [UpdatesLike::ConnectionClosed]));
    assert!(matches!(
        waiting.await.unwrap(),
        Err(InvocationError::Dropped("response could not be recovered"))
    ));
    assert!(sender.requests.is_empty());

    let mut updates = Vec::new();
    let request = sender.state.mtp.request.unwrap();
    sender.process_mtp_buffer(
        vec![mtp::Deserialization::ResponseUnavailable {
            target: mtp::RecoveryTarget::SaltRefresh(request),
        }],
        &mut updates,
    );
    assert!(updates.is_empty());
    sender.process_mtp_buffer(
        vec![mtp::Deserialization::ResponseUnavailable {
            target: mtp::RecoveryTarget::Request(request),
        }],
        &mut updates,
    );
    assert!(matches!(
        updates.as_slice(),
        [UpdatesLike::ConnectionClosed]
    ));
}

static DECODES: AtomicUsize = AtomicUsize::new(0);
struct Counted;
impl Deserializable for Counted {
    fn deserialize(cursor: &mut tl::Cursor<'_>) -> Result<Self, tl::deserialize::Error> {
        DECODES.fetch_add(1, Ordering::SeqCst);
        let _ = u32::deserialize(cursor)?;
        Ok(Self)
    }
}

#[test]
fn setup_results_are_limited_before_the_typed_decoder_runs() {
    DECODES.store(0, Ordering::SeqCst);
    assert!(matches!(
        decode_response::<Counted>(&[0; 8], 4),
        Err(mtp::DeserializeError::LimitExceeded)
    ));
    assert_eq!(DECODES.load(Ordering::SeqCst), 0);
    decode_response::<Counted>(&[0; 4], 4).unwrap();
    assert_eq!(DECODES.load(Ordering::SeqCst), 1);
}

#[test]
fn setup_results_and_updates_reject_unconsumed_bytes() {
    let mut pong = tl::enums::Pong::Pong(tl::types::Pong {
        msg_id: 1,
        ping_id: 2,
    })
    .to_bytes();
    pong.extend_from_slice(&[0; 4]);
    assert!(matches!(
        decode_response::<tl::enums::Pong>(&pong, 1024),
        Err(mtp::DeserializeError::TrailingData)
    ));
    let mut update =
        tl::enums::messages::AffectedMessages::Messages(tl::types::messages::AffectedMessages {
            pts: 1,
            pts_count: 1,
        })
        .to_bytes();
    assert!(matches!(
        decode_updates_exact(&update),
        Ok(UpdatesLike::AffectedMessages(_))
    ));
    update.extend_from_slice(&[0; 4]);
    assert!(decode_updates_exact(&update).is_err());
}

#[tokio::test]
async fn local_serialization_failure_drains_requests_without_writing() {
    struct Exhausted;
    impl mtp::Mtp for Exhausted {
        fn push(
            &mut self,
            _: &mut grammers_mtproto::DequeBuffer<u8>,
            _: &[u8],
        ) -> Option<grammers_mtproto::MsgId> {
            None
        }

        fn finalize(
            &mut self,
            _: &mut grammers_mtproto::DequeBuffer<u8>,
        ) -> Option<grammers_mtproto::MsgId> {
            None
        }

        fn deserialize(
            &mut self,
            _: &mut [u8],
        ) -> Result<Vec<mtp::Deserialization>, mtp::DeserializeError> {
            unreachable!("the fixture has no incoming packets")
        }

        fn serialize_error(&self) -> Option<mtp::SerializeError> {
            Some(mtp::SerializeError::MessageIdsExhausted)
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut sender = Sender::connect(
        grammers_mtproto::transport::Full::new(),
        Exhausted,
        ServerAddr::Tcp {
            address: listener.local_addr().unwrap(),
        },
    )
    .await
    .unwrap();
    let (mut socket, _) = listener.accept().await.unwrap();
    let (reply, waiting) = oneshot::channel();
    sender.enqueue_body(tl::functions::Ping { ping_id: 1 }.to_bytes(), reply);
    let id = mtp::Mtp::push(
        &mut mtp::Plain::new(),
        &mut grammers_mtproto::DequeBuffer::with_capacity(64, 64),
        &tl::functions::Ping { ping_id: 2 }.to_bytes(),
    )
    .unwrap();
    let (sent_reply, sent_waiting) = oneshot::channel();
    sender.requests.push(Request {
        body: tl::functions::Ping { ping_id: 2 }.to_bytes(),
        state: RequestState::Sent(MsgIdPair::new(id)),
        result: sent_reply,
        remaining_setup_repairs: 0,
    });

    let failure = tokio::time::timeout(std::time::Duration::from_secs(2), sender.step())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        failure,
        ReadError::Serialize(mtp::SerializeError::MessageIdsExhausted)
    ));
    assert!(
        failure
            .to_string()
            .contains("message identifiers exhausted")
    );
    assert!(sender.requests.is_empty());
    assert!(matches!(
        waiting.await.unwrap(),
        Err(InvocationError::UnsentSerialization(
            mtp::SerializeError::MessageIdsExhausted
        ))
    ));
    assert!(matches!(
        sent_waiting.await.unwrap(),
        Err(InvocationError::SerializationInterrupted(
            mtp::SerializeError::MessageIdsExhausted
        ))
    ));
    assert!(matches!(
        sender.invoke(&tl::functions::Ping { ping_id: 3 }).await,
        Err(InvocationError::UnsentSerialization(
            mtp::SerializeError::MessageIdsExhausted
        ))
    ));

    sender.disconnect().await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn rpc_responses_and_own_updates_require_a_sent_request() {
    use mtp::Deserialization::{BadMessage, Failure, OwnUpdate, RpcResult};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut sender = Sender::connect(
        grammers_mtproto::transport::Full::new(),
        mtp::Encrypted::build().finish([7; 256]),
        ServerAddr::Tcp {
            address: listener.local_addr().unwrap(),
        },
    )
    .await
    .unwrap();
    let (_socket, _) = listener.accept().await.unwrap();
    let [mut waiting, mut waiting_for_failure] = [1, 2].map(|id| {
        let (reply, waiting) = oneshot::channel();
        let request = tl::functions::messages::DeleteMessages {
            revoke: false,
            id: vec![id],
        };
        sender.enqueue_body(request.to_bytes(), reply);
        waiting
    });
    sender.try_fill_write();
    let buffered_pair = |index: usize| {
        let RequestState::Serialized(pair) = &sender.requests[index].state else {
            panic!("expected a buffered request");
        };
        (pair.msg_id, pair.container_msg_id)
    };
    let (msg_id, container_msg_id) = buffered_pair(0);
    let (other_msg_id, other_container_msg_id) = buffered_pair(1);
    assert_ne!(msg_id, container_msg_id);
    assert_ne!(msg_id, other_msg_id);
    assert_eq!(container_msg_id, other_container_msg_id);
    let body =
        tl::enums::messages::AffectedMessages::Messages(tl::types::messages::AffectedMessages {
            pts: 1,
            pts_count: 1,
        })
        .to_bytes();
    let matched_reply = || {
        vec![
            OwnUpdate {
                msg_id,
                update: body.clone(),
            },
            RpcResult(mtp::RpcResult {
                msg_id,
                body: body.clone(),
            }),
        ]
    };
    let failed_reply = |msg_id| {
        Failure(mtp::DeserializationFailure {
            msg_id,
            error: mtp::DeserializeError::LimitExceeded,
        })
    };
    let mut updates = Vec::new();
    sender.process_mtp_buffer(matched_reply(), &mut updates);
    sender.process_mtp_buffer(
        vec![
            failed_reply(msg_id),
            BadMessage(mtp::BadMessage {
                msg_id: container_msg_id,
                code: 48,
            }),
        ],
        &mut updates,
    );
    assert!(updates.is_empty());
    assert!(matches!(waiting.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(
        waiting_for_failure.try_recv(),
        Err(TryRecvError::Empty)
    ));
    assert!(
        sender
            .requests
            .iter()
            .all(|request| matches!(request.state, RequestState::Serialized(_)))
    );

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while sender
            .requests
            .iter()
            .any(|request| matches!(request.state, RequestState::Serialized(_)))
        {
            sender.step().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(
        sender
            .requests
            .iter()
            .all(|request| matches!(request.state, RequestState::Sent(_)))
    );
    sender.process_mtp_buffer(matched_reply(), &mut updates);
    sender.process_mtp_buffer(matched_reply(), &mut updates);
    sender.process_mtp_buffer(vec![failed_reply(msg_id)], &mut updates);
    assert!(matches!(
        updates.as_slice(),
        [UpdatesLike::AffectedMessages(_)]
    ));
    assert_eq!(waiting.try_recv().unwrap().unwrap(), body);
    assert!(matches!(
        waiting_for_failure.try_recv(),
        Err(TryRecvError::Empty)
    ));

    sender.process_mtp_buffer(
        vec![
            BadMessage(mtp::BadMessage {
                msg_id: container_msg_id,
                code: 48,
            }),
            failed_reply(other_msg_id),
            failed_reply(other_msg_id),
        ],
        &mut updates,
    );
    assert!(matches!(
        updates.as_slice(),
        [
            UpdatesLike::AffectedMessages(_),
            UpdatesLike::MalformedUpdates
        ]
    ));
    assert!(matches!(
        waiting_for_failure.try_recv().unwrap(),
        Err(InvocationError::Deserialize(
            mtp::DeserializeError::LimitExceeded
        ))
    ));
    assert!(sender.requests.is_empty());
}

#[tokio::test]
async fn protocol_rejection_repairs_are_reserved_for_setup_and_bounded() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut sender = Sender::connect(
        grammers_mtproto::transport::Full::new(),
        mtp::Plain::new(),
        ServerAddr::Tcp {
            address: listener.local_addr().unwrap(),
        },
    )
    .await
    .unwrap();
    let (_socket, _) = listener.accept().await.unwrap();
    let ping = tl::functions::Ping { ping_id: 1 }.to_bytes();
    let msg_id = mtp::Mtp::push(
        &mut mtp::Plain::new(),
        &mut grammers_mtproto::DequeBuffer::with_capacity(64, 64),
        &ping,
    )
    .unwrap();

    let (application_reply, mut application_result) = oneshot::channel();
    let (setup_reply, mut setup_result) = oneshot::channel();
    sender.enqueue_body(ping.clone(), application_reply);
    sender.enqueue(ping, setup_reply, 2);

    for remaining in (0..=2).rev() {
        for request in &mut sender.requests {
            request.state = RequestState::Sent(MsgIdPair::new(msg_id));
        }
        sender.process_bad_message(mtp::BadMessage { msg_id, code: 48 });
        if remaining > 0 {
            assert_eq!(sender.requests.len(), 1);
            assert!(matches!(
                sender.requests[0].state,
                RequestState::NotSerialized
            ));
            assert_eq!(sender.requests[0].remaining_setup_repairs, remaining - 1);
            assert!(matches!(setup_result.try_recv(), Err(TryRecvError::Empty)));
        }
    }
    assert!(matches!(
        application_result.try_recv(),
        Ok(Err(InvocationError::Dropped(_)))
    ));
    assert!(matches!(
        setup_result.try_recv(),
        Ok(Err(InvocationError::Dropped(_)))
    ));
    assert!(sender.requests.is_empty());
}

#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
struct Fields(Vec<String>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{field}={value:?}"));
    }
}
impl tracing::Subscriber for Captured {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        self.0.lock().unwrap().extend(fields.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn real_sender_rpc_error_logging_discards_remote_descriptions() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut sender = Sender::connect(
        grammers_mtproto::transport::Full::new(),
        mtp::Encrypted::build().finish([7; 256]),
        ServerAddr::Tcp { address },
    )
    .await
    .unwrap();
    let (_socket, _) = listener.accept().await.unwrap();
    let msg_id = mtp::Mtp::push(
        &mut mtp::Plain::new(),
        &mut grammers_mtproto::DequeBuffer::with_capacity(64, 64),
        &tl::functions::Ping { ping_id: 1 }.to_bytes(),
    )
    .unwrap();
    for code in [400, 12345] {
        let (reply, waiting) = oneshot::channel();
        sender.requests.push(Request {
            body: tl::functions::auth::SignIn {
                phone_number: "+15555550100".into(),
                phone_code_hash: "synthetic_hash".into(),
                phone_code: Some("12345".into()),
                email_verification: None,
            }
            .to_bytes(),
            state: RequestState::Sent(MsgIdPair::new(msg_id)),
            result: reply,
            remaining_setup_repairs: 0,
        });
        let captured = Captured::default();
        tracing::subscriber::with_default(captured.clone(), || {
            sender.process_error(mtp::RpcResultError {
                msg_id,
                error: tl::types::RpcError {
                    error_code: code,
                    error_message: "PRIVATE_PHONE_CODE_12345".into(),
                },
            });
        });
        assert!(matches!(
            waiting.await.unwrap(),
            Err(InvocationError::Rpc(_))
        ));
        let diagnostics = captured.0.lock().unwrap().join("\n");
        assert!(
            diagnostics.contains("got rpc error"),
            "the production event must be captured"
        );
        if code == 400 {
            assert!(diagnostics.contains("400"));
        }
        assert!(!diagnostics.contains("PRIVATE_PHONE_CODE"));
        assert!(!diagnostics.contains("12345"));
        assert!(!diagnostics.contains("synthetic_hash"));
    }
    sender.disconnect().await.unwrap();
}
