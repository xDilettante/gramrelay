// Local setup/update decoder regressions. No sockets or credentials.
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
