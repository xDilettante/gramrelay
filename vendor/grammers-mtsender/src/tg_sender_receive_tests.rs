// Local decoder/diagnostic regressions. Synthetic credentials and localhost only.
use super::*;
use std::sync::atomic::AtomicUsize;

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
