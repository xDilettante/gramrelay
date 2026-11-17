use super::*;
use crate::{ClientErrorKind, Outcome};

#[test]
fn owned_bytes_are_hashed_without_copy_and_read_back_in_bounded_parts() {
    let data = b"hello world".to_vec();
    let pointer = data.as_ptr();
    let mut prepared = Prepared::open(Source::bytes(data), 4, 1024, None, &|| Ok(())).unwrap();
    assert_eq!(prepared.bytes_pointer(), Some(pointer));
    assert_eq!(prepared.size(), 11);
    assert_eq!(prepared.md5_checksum(), "5eb63bbbe01eeed093cb22bb8f5acdc3");
    let mut restored = Vec::new();
    while prepared.has_remaining() {
        let (next, chunk) = prepared.read_next(&|| Ok(())).unwrap();
        assert!(chunk.len() <= 4);
        restored.extend_from_slice(&chunk);
        prepared = next;
    }
    let stats = prepared.finish(&|| Ok(())).unwrap();
    assert_eq!(restored, b"hello world");
    assert_eq!(stats.size, 11);
    let expected: [u8; 32] = sha2::Sha256::digest(b"hello world").into();
    assert_eq!(stats.sha256, expected);
}

#[test]
fn regular_file_uses_one_handle_and_rejects_content_changes_before_final_dispatch() {
    for replacement in [
        b"world".as_slice(),
        b"tiny".as_slice(),
        b"longer".as_slice(),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private-source.bin");
        std::fs::write(&path, b"hello").unwrap();
        let prepared =
            Prepared::open(Source::path(path.clone()), 4, 1024, None, &|| Ok(())).unwrap();
        std::fs::write(&path, replacement).unwrap();
        let result = (|| {
            let mut prepared = prepared;
            while prepared.has_remaining() {
                let (next, _) = prepared.read_next(&|| Ok(()))?;
                prepared = next;
            }
            prepared.finish(&|| Ok(()))
        })();
        assert_eq!(result.err().unwrap().kind(), ClientErrorKind::Integrity);
    }
}

#[test]
fn source_limits_capacity_hash_and_stop_are_checked_without_secret_diagnostics() {
    let mut data = Vec::with_capacity(2048);
    data.extend_from_slice(b"hello");
    let failure = Prepared::open(Source::bytes(data), 4, 1024, None, &|| Ok(()))
        .err()
        .unwrap();
    assert_eq!(failure.outcome(), Outcome::NotSent);
    let failure = Prepared::open(
        Source::bytes(b"private contents".to_vec()),
        4,
        1024,
        Some([0; 32]),
        &|| Ok(()),
    )
    .err()
    .unwrap();
    assert_eq!(failure.kind(), ClientErrorKind::Integrity);
    assert!(!format!("{failure:?}").contains("private"));
    let failure = Prepared::open(Source::bytes(vec![7; 100]), 4, 1024, None, &|| {
        Err(super::super::super::error(
            ClientErrorKind::Closed,
            Outcome::NotSent,
        ))
    })
    .err()
    .unwrap();
    assert_eq!(failure.kind(), ClientErrorKind::Closed);
    let directory = tempfile::tempdir().unwrap();
    let failure = Prepared::open(
        Source::path(directory.path().join("private-missing.bin")),
        4,
        1024,
        None,
        &|| Ok(()),
    )
    .err()
    .unwrap();
    assert!(!format!("{failure:?}").contains("private"));
}
