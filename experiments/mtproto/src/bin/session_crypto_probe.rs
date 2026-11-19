//! Maintained AEAD boundary only; synthetic keys/state, never a private session.
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use zeroize::Zeroizing;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = Zeroizing::new([42u8; 32]);
    let plaintext = Zeroizing::new(
        br#"{"schema_version":1,"synthetic_auth_key":"not-a-Telegram-key","pts":12}"#.to_vec(),
    );
    let context = b"tg-mtproto/session/v1/profile=offline/api_id=1";
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).unwrap();
    let mut first = [0; 24];
    let mut second = [0; 24];
    getrandom::fill(&mut first)?;
    getrandom::fill(&mut second)?;
    assert_ne!(first, second);
    let nonce: XNonce = first.into();
    let payload = Payload {
        msg: plaintext.as_ref(),
        aad: context,
    };
    let encrypted = cipher
        .encrypt(&nonce, payload)
        .expect("synthetic AEAD encrypt");
    let marker = b"not-a-Telegram-key";
    assert!(!encrypted.windows(marker.len()).any(|bytes| bytes == marker));
    let restored = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &encrypted,
                    aad: context,
                },
            )
            .expect("same key/context"),
    );
    assert_eq!(restored.as_slice(), plaintext.as_slice());
    assert!(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &encrypted,
                    aad: b"different-profile"
                }
            )
            .is_err()
    );
    let wrong = XChaCha20Poly1305::new_from_slice(&[43; 32]).unwrap();
    assert!(
        wrong
            .decrypt(
                &nonce,
                Payload {
                    msg: &encrypted,
                    aad: context
                }
            )
            .is_err()
    );
    let mut changed = encrypted;
    changed[0] ^= 1;
    assert!(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &changed,
                    aad: context
                }
            )
            .is_err()
    );
    println!(
        "session crypto: XChaCha20Poly1305 round-trip, fresh nonces, wrong-key/context and tamper rejection verified with synthetic material."
    );
    Ok(())
}
