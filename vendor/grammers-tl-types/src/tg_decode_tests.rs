// Downstream canonical TL-byte-string tests, using no Telegram account.
use crate::deserialize::{Cursor, Error};
use crate::{Deserializable, RawVec, Serializable};

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
    for length in 0..=256 {
        let bytes = vec![42; length];
        assert_eq!(Vec::<u8>::from_bytes(&bytes.to_bytes()).unwrap(), bytes);
    }
}

#[test]
fn boxed_and_bare_vectors_share_one_cursor_allocation_budget() {
    let first = vec![1i32, 2, 3];
    let second = RawVec(vec![4i32, 5, 6]);
    let mut encoded = first.to_bytes();
    encoded.extend(second.to_bytes());
    let mut cursor = Cursor::with_budgets(&encoded, 20, 6);
    assert_eq!(Vec::<i32>::deserialize(&mut cursor), Ok(first));
    assert_eq!(
        RawVec::<i32>::deserialize(&mut cursor),
        Err(Error::LimitExceeded)
    );
}

#[test]
fn vector_storage_and_nested_owned_fields_use_the_same_allocation_budget() {
    let value = vec![String::from("abc"), String::from("test")];
    let bytes = value.to_bytes();
    let required = 2 * std::mem::size_of::<String>() + 7;
    let mut too_small = Cursor::with_budgets(&bytes, required - 1, 2);
    assert_eq!(
        Vec::<String>::deserialize(&mut too_small),
        Err(Error::LimitExceeded)
    );
    let mut exact = Cursor::with_budgets(&bytes, required, 2);
    assert_eq!(Vec::<String>::deserialize(&mut exact), Ok(value));
    assert_eq!(exact.pos(), bytes.len());
}

#[test]
fn zero_sized_bare_items_still_use_the_cursor_work_budget() {
    #[derive(Debug, PartialEq)]
    struct Empty;
    impl Deserializable for Empty {
        fn deserialize(_: &mut Cursor<'_>) -> Result<Self, Error> {
            Ok(Self)
        }
    }

    let counts = [2u32.to_le_bytes(), 2u32.to_le_bytes()].concat();
    let mut cursor = Cursor::with_budgets(&counts, 0, 3);
    assert_eq!(
        RawVec::<Empty>::deserialize(&mut cursor),
        Ok(RawVec(vec![Empty, Empty]))
    );
    assert_eq!(
        RawVec::<Empty>::deserialize(&mut cursor),
        Err(Error::LimitExceeded)
    );
}

#[test]
fn lossy_utf8_matches_the_standard_library_and_charges_output_bytes() {
    for data in [
        "текст".as_bytes(),
        &[0xc2, b'a', 0xf0, 0x90, 0x80],
        &[0xff, 0xff, b'x'],
        &[],
    ] {
        let bytes = data.to_vec().to_bytes();
        let expected = String::from_utf8_lossy(data);
        let mut exact = Cursor::with_budgets(&bytes, expected.len(), 0);
        assert_eq!(String::deserialize(&mut exact).unwrap(), expected);
        if !expected.is_empty() {
            let mut small = Cursor::with_budgets(&bytes, expected.len() - 1, 0);
            assert_eq!(String::deserialize(&mut small), Err(Error::LimitExceeded));
        }
    }
}

#[test]
fn byte_read_checks_input_first_and_keeps_failed_append_atomic() {
    let mut cursor = Cursor::with_budgets(b"abc", 2, 0);
    assert_eq!(cursor.read_bytes(10), Err(Error::UnexpectedEof));
    let mut output = Vec::new();
    assert_eq!(cursor.read_to_end(&mut output), Err(Error::LimitExceeded));
    assert!(output.is_empty());
    assert_eq!(cursor.pos(), 0);
    assert_eq!(cursor.read_bytes(2), Ok(b"ab".to_vec()));
    assert_eq!(cursor.read_bytes(1), Err(Error::LimitExceeded));
}

#[test]
fn changing_vector_capacity_charges_each_allocation() {
    let values: Vec<i64> = (0..9).collect();
    let bytes = values.to_bytes();
    let needed = (4 + 8 + 9) * std::mem::size_of::<i64>();
    let mut exact = Cursor::with_budgets(&bytes, needed, values.len());
    assert_eq!(Vec::<i64>::deserialize(&mut exact), Ok(values.clone()));
    let mut one_short = Cursor::with_budgets(&bytes, needed - 1, values.len());
    assert_eq!(
        Vec::<i64>::deserialize(&mut one_short),
        Err(Error::LimitExceeded)
    );
}
