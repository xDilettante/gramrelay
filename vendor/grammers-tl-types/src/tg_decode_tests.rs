// Downstream canonical TL-byte-string tests, using no Telegram account.
use crate::Deserializable;

#[test]
fn reserved_length_prefix_and_nonzero_padding_are_rejected() {
    assert!(Vec::<u8>::from_bytes(&[1, 42, 1, 0]).is_err());
    let mut reserved = vec![255];
    reserved.extend([42; 255]);
    assert!(Vec::<u8>::from_bytes(&reserved).is_err());
}

#[test]
fn a_long_length_prefix_cannot_encode_a_short_string() {
    assert!(Vec::<u8>::from_bytes(&[254, 1, 0, 0, 42, 0, 0, 0]).is_err());
}

#[test]
fn every_short_length_and_the_long_boundary_roundtrip_canonically() {
    use crate::Serializable;
    for length in 0..=256 {
        let bytes = vec![42; length];
        assert_eq!(Vec::<u8>::from_bytes(&bytes.to_bytes()).unwrap(), bytes);
    }
}
