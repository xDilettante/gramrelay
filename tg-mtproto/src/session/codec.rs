use super::{
    SessionBinding, SessionErrorKind, SessionKey, SessionOptions,
    data::Snapshot,
    types::{Result, error, storage},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Write};
use zeroize::{Zeroize, Zeroizing};

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
    let plaintext = snapshot_plaintext(snapshot, options.max_snapshot_bytes)?;
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
    // Probe only the authenticated payload version, then decode exactly that
    // structure. Never accept invalid new data through a legacy fallback.
    #[derive(Deserialize)]
    struct Version {
        version: u8,
    }
    let version: Version =
        serde_json::from_slice(&plaintext).map_err(|_| error(SessionErrorKind::Authentication))?;
    let snapshot = match version.version {
        1 => decode_strict::<LegacySnapshot>(&plaintext)?.upgrade()?,
        2 => decode_strict::<SnapshotV2>(&plaintext)?.upgrade()?,
        3 => decode_strict::<Snapshot>(&plaintext)?,
        _ => return Err(error(SessionErrorKind::UnsupportedSchema)),
    };
    snapshot.validate(options)?;
    Ok(snapshot)
}

fn decode_strict<T: DeserializeOwned>(plaintext: &[u8]) -> Result<T> {
    let mut deserializer = serde_json::Deserializer::from_slice(plaintext);
    let mut unknown = false;
    let value = serde_ignored::deserialize(&mut deserializer, |_| unknown = true)
        .map_err(|_| error(SessionErrorKind::Authentication))?;
    deserializer
        .end()
        .map_err(|_| error(SessionErrorKind::Authentication))?;
    if unknown {
        return Err(error(SessionErrorKind::Authentication));
    }
    Ok(value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySnapshot {
    version: u8,
    home_dc: i32,
    datacenters: Vec<grammers_session::types::DcOption>,
    peers: Vec<grammers_session::types::PeerInfo>,
    #[serde(deserialize_with = "super::updates::required_option")]
    acknowledged: Option<grammers_session::types::UpdatesState>,
    #[serde(deserialize_with = "super::updates::required_option")]
    account: Option<super::AccountIdentity>,
}
impl LegacySnapshot {
    fn upgrade(mut self) -> Result<Snapshot> {
        if self.version != 1 {
            return Err(error(SessionErrorKind::UnsupportedSchema));
        }
        Ok(Snapshot {
            version: 3,
            quarantined: false,
            home_dc: self.home_dc,
            datacenters: std::mem::take(&mut self.datacenters),
            peers: std::mem::take(&mut self.peers),
            acknowledged: self.acknowledged.take(),
            account: self.account.take(),
            updates: Default::default(),
        })
    }
}
impl Drop for LegacySnapshot {
    fn drop(&mut self) {
        for dc in &mut self.datacenters {
            if let Some(key) = &mut dc.auth_key {
                key.zeroize();
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotV2 {
    version: u8,
    quarantined: bool,
    home_dc: i32,
    datacenters: Vec<grammers_session::types::DcOption>,
    peers: Vec<grammers_session::types::PeerInfo>,
    #[serde(deserialize_with = "super::updates::required_option")]
    acknowledged: Option<grammers_session::types::UpdatesState>,
    #[serde(deserialize_with = "super::updates::required_option")]
    account: Option<super::AccountIdentity>,
}
impl SnapshotV2 {
    fn upgrade(mut self) -> Result<Snapshot> {
        if self.version != 2 {
            return Err(error(SessionErrorKind::UnsupportedSchema));
        }
        Ok(Snapshot {
            version: 3,
            quarantined: self.quarantined,
            home_dc: self.home_dc,
            datacenters: std::mem::take(&mut self.datacenters),
            peers: std::mem::take(&mut self.peers),
            acknowledged: self.acknowledged.take(),
            account: self.account.take(),
            updates: Default::default(),
        })
    }
}
impl Drop for SnapshotV2 {
    fn drop(&mut self) {
        for dc in &mut self.datacenters {
            if let Some(key) = &mut dc.auth_key {
                key.zeroize();
            }
        }
    }
}
struct Buffer {
    bytes: Zeroizing<Vec<u8>>,
    limit: usize,
}
fn snapshot_plaintext(snapshot: &Snapshot, limit: usize) -> Result<Buffer> {
    struct Size {
        length: usize,
        limit: usize,
    }
    impl Write for Size {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.length = self
                .length
                .checked_add(bytes.len())
                .filter(|length| *length <= self.limit)
                .ok_or_else(|| io::Error::other("snapshot limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // Counting the immutable, concrete snapshot needs no plaintext allocation.
    // Allocate once at its exact encoded size: no freed secret copies from
    // growth, and no full-limit wipe on every small metadata write.
    let mut size = Size { length: 0, limit };
    serde_json::to_writer(&mut size, snapshot).map_err(|_| error(SessionErrorKind::Capacity))?;
    let mut plaintext = Buffer::new(size.length);
    serde_json::to_writer(&mut plaintext, snapshot)
        .map_err(|_| error(SessionErrorKind::Capacity))?;
    Ok(plaintext)
}
impl Buffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Zeroizing::new(Vec::with_capacity(limit)),
            limit,
        }
    }
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
    fn plaintext_writer_keeps_one_allocation_until_its_limit_and_refuses_overflow() {
        let mut buffer = Buffer::new(1024);
        buffer.write_all(b"private").unwrap();
        let pointer = buffer.bytes.as_ptr();
        let capacity = buffer.bytes.capacity();
        for _ in 0..1017 {
            buffer.write_all(&[7]).unwrap();
        }
        assert_eq!(buffer.bytes.as_ptr(), pointer);
        assert_eq!(buffer.bytes.capacity(), capacity);
        assert!(buffer.write_all(&[7]).is_err());
        assert_eq!(buffer.bytes.len(), 1024);
        assert_eq!(&buffer.bytes[..7], b"private");
    }
    #[test]
    fn small_snapshot_encoding_does_not_allocate_or_wipe_the_full_configured_limit() {
        let snapshot = Snapshot::default();
        let buffer = snapshot_plaintext(&snapshot, 64 * 1024 * 1024).unwrap();
        assert!(buffer.bytes.capacity() < 4096);
        assert_eq!(&*buffer.bytes, &serde_json::to_vec(&snapshot).unwrap());
        assert!(snapshot_plaintext(&snapshot, buffer.bytes.len() - 1).is_err());
    }

    fn authenticated_value(value: &serde_json::Value) -> Result<Snapshot> {
        authenticated_plaintext(&serde_json::to_vec(value).unwrap())
    }
    fn authenticated_plaintext(plaintext: &[u8]) -> Result<Snapshot> {
        let binding = SessionBinding::new("demo", 1).unwrap();
        let key = SessionKey::from_bytes([7; 32]);
        let id = [1; 16];
        let nonce = [2; 24];
        let ciphertext = XChaCha20Poly1305::new_from_slice(key.0.as_ref())
            .unwrap()
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &context(&id, 0, &binding).unwrap(),
                },
            )
            .unwrap();
        decrypt(
            nonce,
            &ciphertext,
            &id,
            0,
            &binding,
            &key,
            &SessionOptions::default(),
        )
    }

    fn journal_value() -> serde_json::Value {
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
            pts: 10,
            qts: 0,
            date: 100,
            seq: 10,
            channels: vec![ChannelState { id: 9, pts: 10 }],
        });
        let mut value = serde_json::to_value(snapshot).unwrap();
        let checkpoint = serde_json::json!({ "pts":11,"qts":0,"date":101,"seq":11,"channels":[{"id":9,"pts":11}] });
        value["updates"] = serde_json::json!({
            "serial":2,"last_ack":{"serial":1,"nonce":vec![1u8;16]},"gap":null,
            "pending":{"id":{"serial":2,"nonce":vec![2u8;16]},"draft":{
                "account":value["account"],"kind":"events","base":value["acknowledged"],"checkpoint":checkpoint,"payload":[1,2,3]
            }}
        });
        value
    }

    #[test]
    fn v2_upgrade_preserves_quarantine_keys_identity_and_ack_without_fallback() {
        let mut value = journal_value();
        value.as_object_mut().unwrap().remove("updates");
        value["version"] = 2.into();
        value["quarantined"] = true.into();
        let restored = authenticated_value(&value).unwrap();
        assert_eq!(restored.version, 3);
        assert!(restored.quarantined);
        assert!(
            restored.acknowledged
                == Some(serde_json::from_value(value["acknowledged"].clone()).unwrap())
        );
        assert!(
            restored.account.is_some()
                && restored.updates.pending.is_none()
                && restored.updates.gap.is_none()
        );
        value["updates"] = serde_json::json!({});
        assert!(
            matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication)
        );
    }

    #[test]
    fn journal_required_null_keys_unknown_fields_and_duplicates_fail_closed() {
        let valid = journal_value();
        assert!(authenticated_value(&valid).is_ok());
        for (parent, field) in [
            ("", "account"),
            ("", "acknowledged"),
            ("", "updates"),
            ("/updates", "pending"),
            ("/updates", "gap"),
            ("/updates", "last_ack"),
            ("/updates/pending/draft", "base"),
        ] {
            let mut value = valid.clone();
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication),
                "{parent}/{field}"
            );
        }
        for parent in [
            "/updates",
            "/updates/pending",
            "/updates/pending/id",
            "/updates/pending/draft",
            "/updates/pending/draft/checkpoint",
            "/updates/pending/draft/checkpoint/channels/0",
        ] {
            let mut value = valid.clone();
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), true.into());
            assert!(
                matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication),
                "{parent}"
            );
        }
        let duplicate = serde_json::to_string(&valid).unwrap().replacen(
            "\"serial\":2",
            "\"serial\":2,\"serial\":2",
            1,
        );
        assert!(
            matches!(authenticated_plaintext(duplicate.as_bytes()), Err(error) if error.kind()==SessionErrorKind::Authentication)
        );
        for version in [1, 2] {
            for missing in ["account", "acknowledged"] {
                let mut value = valid.clone();
                value["version"] = version.into();
                value.as_object_mut().unwrap().remove("updates");
                if version == 1 {
                    value.as_object_mut().unwrap().remove("quarantined");
                }
                value.as_object_mut().unwrap().remove(missing);
                assert!(
                    matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication)
                );
            }
        }
    }

    #[test]
    fn journal_rejects_unreachable_ordinals_foreign_base_and_invalid_resync() {
        let valid = journal_value();
        for case in 0..8 {
            let mut value = valid.clone();
            match case {
                0 => value["updates"]["pending"]["id"]["serial"] = 1.into(),
                1 => {
                    value["updates"]["serial"] = 3.into();
                    value["updates"]["last_ack"]["serial"] = 3.into();
                }
                2 => value["updates"]["pending"]["draft"]["base"]["pts"] = 9.into(),
                3 => value["updates"]["pending"]["draft"]["account"]["id"] = 43.into(),
                4 => {
                    value["updates"]["pending"]["draft"]["kind"] =
                        serde_json::json!({"resync":{"gap":{"serial":1,"nonce":vec![1u8;16]}}})
                }
                5 => {
                    value["updates"]["gap"] = serde_json::json!({"id":{"serial":1,"nonce":vec![3u8;16]},"scope":"account","reason":"queue_overflow"})
                }
                6 => {
                    value["updates"]["serial"] = 3.into();
                    value["updates"]["pending"]["id"]["serial"] = 3.into();
                    value["updates"]["gap"] = serde_json::json!({"id":{"serial":2,"nonce":vec![3u8;16]},"scope":"account","reason":"queue_overflow"});
                }
                _ => {
                    let id = serde_json::json!({"serial":3,"nonce":vec![3u8;16]});
                    value["updates"]["serial"] = 3.into();
                    value["updates"]["pending"]["draft"]["kind"] =
                        serde_json::json!({"resync":{"gap":id}});
                    value["updates"]["pending"]["draft"]["payload"] = serde_json::json!([]);
                    value["updates"]["gap"] =
                        serde_json::json!({"id":id,"scope":"account","reason":"queue_overflow"});
                }
            }
            assert!(
                matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication),
                "case {case}"
            );
        }
    }

    #[test]
    fn legacy_payload_upgrades_without_changing_its_identity_or_keys() {
        let mut snapshot = Snapshot::default();
        snapshot.datacenters[0].auth_key = Some([0xa5; 256]);
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
            pts: 10,
            ..UpdatesState::default()
        });
        let mut value = serde_json::to_value(&snapshot).unwrap();
        value["version"] = 1.into();
        value.as_object_mut().unwrap().remove("quarantined");
        value.as_object_mut().unwrap().remove("updates");
        let restored = authenticated_value(&value).unwrap();
        assert_eq!(restored.version, 3);
        assert!(!restored.quarantined);
        assert!(restored.datacenters == snapshot.datacenters);
        assert_eq!(restored.account, snapshot.account);
        assert!(restored.acknowledged == snapshot.acknowledged);
        value["datacenters"][0]["future_field"] = true.into();
        assert!(
            matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication)
        );
    }

    #[test]
    fn v2_quarantine_is_required_and_never_defaults_away() {
        for invalid in [None, Some(serde_json::Value::Null), Some("false".into())] {
            let mut value = serde_json::to_value(Snapshot::default()).unwrap();
            value["version"] = 2.into();
            value.as_object_mut().unwrap().remove("updates");
            value.as_object_mut().unwrap().remove("quarantined");
            if let Some(invalid) = invalid {
                value["quarantined"] = invalid;
            }
            assert!(
                matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication)
            );
        }
        let mut value = serde_json::to_value(Snapshot::default()).unwrap();
        value["version"] = 4.into();
        assert!(
            matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::UnsupportedSchema)
        );
        value["version"] = 1.into();
        value["quarantined"] = false.into();
        assert!(
            matches!(authenticated_value(&value), Err(error) if error.kind()==SessionErrorKind::Authentication)
        );
    }

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
