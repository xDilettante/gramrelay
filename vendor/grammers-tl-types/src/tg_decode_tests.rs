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

fn sample(seed: &mut u32) -> usize {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as usize
}

#[test]
fn seeded_collection_cases_cover_capacity_and_encoded_boundaries() {
    let mut seed = 0x219d_cea3;
    for _ in 0..384 {
        let length = sample(&mut seed) % 600;
        let data = (0..length)
            .map(|_| sample(&mut seed) as u8)
            .collect::<Vec<_>>();
        let encoded = data.to_bytes();
        let mut cursor = Cursor::with_budgets(&encoded, length, 0);
        assert_eq!(Vec::<u8>::deserialize(&mut cursor), Ok(data));
        if length != 0 {
            let mut short = Cursor::with_budgets(&encoded, length - 1, 0);
            assert_eq!(
                Vec::<u8>::deserialize(&mut short),
                Err(Error::LimitExceeded)
            );
        }
        let truncated = sample(&mut seed) % encoded.len();
        assert!(Vec::<u8>::from_bytes(&encoded[..truncated]).is_err());

        let count = sample(&mut seed) % 64;
        let numbers = (0..count)
            .map(|_| sample(&mut seed) as i32)
            .collect::<Vec<_>>();
        let encoded = numbers.to_bytes();
        let item_budget = sample(&mut seed) % 64;
        let mut cursor = Cursor::with_budgets(&encoded, 8192, item_budget);
        let expected = if count <= item_budget {
            Ok(numbers)
        } else {
            Err(Error::LimitExceeded)
        };
        assert_eq!(Vec::<i32>::deserialize(&mut cursor), expected);
    }
}

#[cfg(feature = "tl-mtproto")]
#[test]
fn mtproto_tls_scope_accepts_an_empty_leaf_at_the_depth_limit() {
    use crate::deserialize::NESTING_BUDGET;
    use crate::{enums::TlsBlock, types::TlsBlockScope};

    let mut value = TlsBlock::Scope(TlsBlockScope {
        entries: Vec::new(),
    });
    for _ in 0..NESTING_BUDGET {
        value = TlsBlock::Scope(TlsBlockScope {
            entries: vec![value],
        });
    }
    assert!(TlsBlock::from_bytes(&value.to_bytes()) == Ok(value.clone()));
    let deeper = TlsBlock::Scope(TlsBlockScope {
        entries: vec![value],
    });
    assert!(matches!(
        TlsBlock::from_bytes(&deeper.to_bytes()),
        Err(Error::LimitExceeded)
    ));
}

#[cfg(feature = "tl-api")]
mod generated_recursive_types {
    use super::*;
    use crate::deserialize::NESTING_BUDGET;
    use crate::{enums, types};

    fn rich_text(depth: usize) -> enums::RichText {
        let mut text = enums::RichText::TextEmpty;
        for _ in 0..depth {
            text = enums::RichText::TextBold(Box::new(types::TextBold { text }));
        }
        text
    }

    fn json_list(depth: usize) -> enums::Jsonvalue {
        let mut value = enums::Jsonvalue::JsonArray(types::JsonArray { value: Vec::new() });
        for _ in 0..depth {
            value = enums::Jsonvalue::JsonArray(types::JsonArray { value: vec![value] });
        }
        value
    }

    fn mixed_page_block(depth: usize) -> enums::PageBlock {
        let mut block = enums::PageBlock::Unsupported;
        for level in 0..depth {
            block = if level % 2 == 0 {
                enums::PageBlock::Cover(Box::new(types::PageBlockCover { cover: block }))
            } else {
                enums::PageBlock::Collage(types::PageBlockCollage {
                    items: vec![block],
                    caption: enums::PageCaption::Caption(types::PageCaption {
                        text: enums::RichText::TextEmpty,
                        credit: enums::RichText::TextEmpty,
                    }),
                })
            };
        }
        block
    }

    fn nested_story(layers: usize) -> enums::StoryItem {
        let mut story = enums::StoryItem::Deleted(types::StoryItemDeleted { id: 1 });
        for id in 2..(layers as i32 + 2) {
            let media = enums::MessageMedia::Story(Box::new(types::MessageMediaStory {
                via_mention: false,
                peer: enums::Peer::User(types::PeerUser { user_id: 2 }),
                id,
                story: Some(story),
            }));
            story = enums::StoryItem::Item(Box::new(types::StoryItem {
                pinned: false,
                public: true,
                close_friends: false,
                min: false,
                noforwards: false,
                edited: false,
                contacts: false,
                selected_contacts: false,
                out: false,
                id,
                date: 20,
                from_id: None,
                fwd_from: None,
                expire_date: 40,
                caption: None,
                entities: None,
                media,
                media_areas: None,
                privacy: None,
                views: None,
                sent_reaction: None,
                albums: None,
                music: None,
            }));
        }
        story
    }

    fn check_depth<T: Serializable + Deserializable + PartialEq>(value: T, depth: usize) {
        let actual = T::from_bytes(&value.to_bytes());
        if depth <= NESTING_BUDGET {
            assert!(actual == Ok(value), "valid value at depth {depth}");
        } else {
            assert!(
                matches!(actual, Err(Error::LimitExceeded)),
                "depth {depth} exceeds the limit"
            );
        }
    }

    #[test]
    fn typed_box_vector_and_mixed_shapes_respect_the_same_depth_on_a_small_thread_stack() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                for depth in [NESTING_BUDGET - 1, NESTING_BUDGET, NESTING_BUDGET + 1] {
                    check_depth(rich_text(depth), depth);
                    check_depth(json_list(depth), depth);
                    check_depth(mixed_page_block(depth), depth);
                }
                let story_layers = NESTING_BUDGET / 2;
                check_depth(nested_story(story_layers), story_layers * 2);
                check_depth(nested_story(story_layers + 1), (story_layers + 1) * 2);
                let mut seed = 0xa029_57b3;
                for _ in 0..384 {
                    let depth = sample(&mut seed) % (NESTING_BUDGET + 4);
                    match sample(&mut seed) % 3 {
                        0 => check_depth(rich_text(depth), depth),
                        1 => check_depth(json_list(depth), depth),
                        _ => check_depth(mixed_page_block(depth), depth),
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn recursive_boxes_charge_each_inner_value() {
        let value = rich_text(3);
        let encoded = value.to_bytes();
        let needed = 3 * std::mem::size_of::<types::TextBold>();
        let mut exact = Cursor::with_budgets(&encoded, needed, 0);
        assert!(enums::RichText::deserialize(&mut exact) == Ok(value));
        let mut short = Cursor::with_budgets(&encoded, needed - 1, 0);
        assert!(matches!(
            enums::RichText::deserialize(&mut short),
            Err(Error::LimitExceeded)
        ));
    }

    #[test]
    fn sibling_values_reuse_depth_but_not_allocation_budget() {
        let value = vec![rich_text(NESTING_BUDGET - 1), rich_text(NESTING_BUDGET - 1)];
        let encoded = value.to_bytes();
        let needed = 2 * std::mem::size_of::<enums::RichText>()
            + 2 * (NESTING_BUDGET - 1) * std::mem::size_of::<types::TextBold>();
        let mut exact = Cursor::with_budgets(&encoded, needed, 2);
        assert!(Vec::<enums::RichText>::deserialize(&mut exact) == Ok(value));
        let mut short = Cursor::with_budgets(&encoded, needed - 1, 2);
        assert!(matches!(
            Vec::<enums::RichText>::deserialize(&mut short),
            Err(Error::LimitExceeded)
        ));
    }
}
