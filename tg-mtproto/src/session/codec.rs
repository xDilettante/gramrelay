use super::{
    SessionBinding, SessionErrorKind, SessionKey, SessionOptions,
    data::Snapshot,
    types::{Result, error, storage},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::Serialize;
use std::io::{self, Write};
use zeroize::Zeroizing;

#[derive(Serialize)]
struct Context<'a> {
    domain: &'static str,
    version: u8,
    storage_id: &'a [u8; 16],
    revision: i64,
    binding: &'a SessionBinding,
}
fn context(id: &[u8; 16], revision: i64, binding: &SessionBinding) -> Result<Vec<u8>> {
    serde_json::to_vec(&Context {
        domain: "tg-mtproto/session",
        version: 1,
        storage_id: id,
        revision,
        binding,
    })
    .map_err(storage)
}
pub(super) fn encrypt(
    snapshot: &Snapshot,
    id: &[u8; 16],
    revision: i64,
    binding: &SessionBinding,
    key: &SessionKey,
    options: &SessionOptions,
) -> Result<([u8; 24], Vec<u8>)> {
    snapshot.validate(options)?;
    let mut plaintext = Buffer {
        bytes: Zeroizing::new(Vec::new()),
        limit: options.max_snapshot_bytes,
    };
    serde_json::to_writer(&mut plaintext, snapshot)
        .map_err(|_| error(SessionErrorKind::Capacity))?;
    let aad = context(id, revision, binding)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.0.as_ref()).map_err(storage)?;
    let mut nonce = [0; 24];
    getrandom::fill(&mut nonce).map_err(storage)?;
    let encrypted = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext.bytes,
                aad: &aad,
            },
        )
        .map_err(storage)?;
    Ok((nonce, encrypted))
}
pub(super) fn decrypt(
    nonce: [u8; 24],
    ciphertext: &[u8],
    id: &[u8; 16],
    revision: i64,
    binding: &SessionBinding,
    key: &SessionKey,
    options: &SessionOptions,
) -> Result<Snapshot> {
    if ciphertext.len() > options.max_snapshot_bytes + 16 {
        return Err(error(SessionErrorKind::Capacity));
    }
    let aad = context(id, revision, binding)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.0.as_ref()).map_err(storage)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| error(SessionErrorKind::Authentication))?,
    );
    let mut deserializer = serde_json::Deserializer::from_slice(&plaintext);
    let mut unknown = false;
    let snapshot: Snapshot = serde_ignored::deserialize(&mut deserializer, |_| unknown = true)
        .map_err(|_| error(SessionErrorKind::Authentication))?;
    deserializer
        .end()
        .map_err(|_| error(SessionErrorKind::Authentication))?;
    if unknown {
        return Err(error(SessionErrorKind::Authentication));
    }
    snapshot.validate(options)?;
    Ok(snapshot)
}
struct Buffer {
    bytes: Zeroizing<Vec<u8>>,
    limit: usize,
}
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.limit)
        {
            return Err(io::Error::other("snapshot limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_session::types::{ChannelState, PeerInfo, UpdatesState};

    #[test]
    fn unknown_nested_snapshot_fields_fail_closed_even_with_a_valid_authentication_tag() {
        let options = SessionOptions::default();
        let binding = SessionBinding::new("demo", 1).unwrap();
        let key = SessionKey::from_bytes([7; 32]);
        let id = [1; 16];
        let nonce = [2; 24];
        let mut snapshot = Snapshot::default();
        snapshot.peers.push(PeerInfo::User {
            id: 42,
            auth: None,
            bot: Some(false),
            is_self: Some(true),
        });
        snapshot.account = Some(super::super::AccountIdentity {
            id: super::super::AccountId::new(42).unwrap(),
            kind: super::super::AccountKind::User,
        });
        snapshot.acknowledged = Some(UpdatesState {
            channels: vec![ChannelState { id: 1, pts: 0 }],
            ..UpdatesState::default()
        });
        for path in [
            "/datacenters/0",
            "/peers/0/User",
            "/acknowledged",
            "/acknowledged/channels/0",
            "/account",
        ] {
            let mut value = serde_json::to_value(&snapshot).unwrap();
            value
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("future_field".into(), true.into());
            let plaintext = serde_json::to_vec(&value).unwrap();
            let ciphertext = XChaCha20Poly1305::new_from_slice(key.0.as_ref())
                .unwrap()
                .encrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: &plaintext,
                        aad: &context(&id, 0, &binding).unwrap(),
                    },
                )
                .unwrap();
            assert!(
                matches!(decrypt(nonce,&ciphertext,&id,0,&binding,&key,&options), Err(error) if error.kind()==SessionErrorKind::Authentication),
                "{path}"
            );
        }
    }
}
