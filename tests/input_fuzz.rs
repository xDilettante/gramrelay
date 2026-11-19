//! Reproducible offline mutations of Telegram JSON and Unicode entity inputs.

use serde::{Serialize, de::DeserializeOwned};
use tg::{
    EffectOutcome, EntityKind, ErrorKind, RichText, Update,
    api::types::{InlineQueryResult, InputMedia, MaybeInaccessibleMessage, MessageEntity},
};

const TEXT_ALPHABET: &[char] = &['a', 'я', '漢', '🦀', '\u{301}', '\u{200d}', '\u{1f680}'];

fn mutate_json<T: DeserializeOwned + Serialize>(seed: &str, rng: &mut fastrand::Rng) {
    let check = |input: &[u8]| {
        let Ok(value) = serde_json::from_slice::<T>(input) else {
            return false;
        };
        let encoded = serde_json::to_vec(&value).unwrap();
        let decoded: T = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
        true
    };
    assert!(check(seed.as_bytes()));
    let mut accepted = 0;
    let mut rejected = 0;
    for _ in 0..1024 {
        let mut input = seed.as_bytes().to_vec();
        for _ in 0..rng.usize(1..=4) {
            if input.is_empty() {
                input.push(rng.u8(..));
                continue;
            }
            let position = rng.usize(..input.len());
            match rng.usize(..5) {
                0 => input[position] = rng.u8(..),
                1 => input.insert(position, rng.u8(..)),
                2 => {
                    input.remove(position);
                }
                3 => input.truncate(position),
                _ => input[position] ^= 1 << rng.u8(..8),
            }
        }
        if check(&input) {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }
    assert!(accepted > 0 && rejected > 0);
}

#[test]
fn seeded_json_mutations_exercise_public_wire_decoders() {
    let mut rng = fastrand::Rng::with_seed(0x74ce_be24);
    mutate_json::<Update>(
        r#"{"update_id":42,"message":{"message_id":7,"date":1,"chat":{"id":2,"type":"private"},"text":"я🦀"},"future":{"items":[1,false,null]}}"#,
        &mut rng,
    );
    mutate_json::<InputMedia>(
        r#"{"type":"photo","media":"source","caption":"🦀","caption_entities":[{"type":"bold","offset":0,"length":2}]}"#,
        &mut rng,
    );
    mutate_json::<InlineQueryResult>(
        r#"{"type":"photo","id":"7","photo_file_id":"file","title":"Привет","future":73}"#,
        &mut rng,
    );
    mutate_json::<MaybeInaccessibleMessage>(
        r#"{"message_id":7,"date":0,"chat":{"id":-7,"type":"group","future":true}}"#,
        &mut rng,
    );
    mutate_json::<tg::api::types::RichText>(
        r#"["я",{"type":"bold","text":[{"type":"italic","text":"🦀"},{"type":"future","payload":{"a":[1]}}]}]"#,
        &mut rng,
    );
}

fn sample_text(rng: &mut fastrand::Rng) -> String {
    (0..rng.usize(1..=12))
        .map(|_| TEXT_ALPHABET[rng.usize(..TEXT_ALPHABET.len())])
        .collect()
}

#[test]
fn seeded_unicode_ranges_match_rust_slices_and_utf16_offsets() {
    let mut rng = fastrand::Rng::with_seed(0x243a_e56c);
    for _ in 0..2048 {
        let text = sample_text(&mut rng);
        let start = rng.usize(..=text.len() + 1);
        let end = rng.usize(..=text.len() + 1);
        let expected = text.get(start..end).filter(|slice| !slice.is_empty());
        match (
            expected,
            RichText::new(&text).entity(start..end, EntityKind::Bold),
        ) {
            (Some(selected), Ok(rich)) => {
                assert_eq!(rich.text(), text);
                assert_eq!(rich.entities().len(), 1);
                assert_eq!(
                    rich.entities()[0].offset,
                    text[..start].encode_utf16().count() as i64
                );
                assert_eq!(
                    rich.entities()[0].length,
                    selected.encode_utf16().count() as i64
                );
                let prefix = sample_text(&mut rng);
                let composed = RichText::new(&prefix).append(rich);
                assert_eq!(composed.text(), format!("{prefix}{text}"));
                assert_eq!(
                    composed.entities()[0].offset,
                    (prefix.encode_utf16().count() + text[..start].encode_utf16().count()) as i64
                );
            }
            (None, Err(error)) => {
                assert_eq!(error.kind(), ErrorKind::Validation);
                assert_eq!(error.outcome(), EffectOutcome::NotSent);
            }
            (expected, result) => panic!("range {start}..{end}: {expected:?}, {result:?}"),
        }
    }
}

#[test]
fn seeded_entity_sets_match_pairwise_style_containment() {
    let mut rng = fastrand::Rng::with_seed(0x0a45_0939);
    for iteration in 0..2048 {
        let text = sample_text(&mut rng);
        let mut boundary = 0;
        let mut boundaries = vec![0];
        for character in text.chars() {
            boundary += character.len_utf16() as i64;
            boundaries.push(boundary);
        }
        let mut entities = Vec::new();
        for _ in 0..rng.usize(1..=5) {
            let start = rng.usize(..boundaries.len() - 1);
            let end = rng.usize(start + 1..boundaries.len());
            let style = ["bold", "italic", "underline"][rng.usize(..3)];
            let offset = boundaries[start];
            let length = boundaries[end] - offset;
            entities.push(MessageEntity::new(style.into(), offset, length));
        }
        if iteration % 3 == 0 {
            let index = rng.usize(..entities.len());
            let entity = &mut entities[index];
            if rng.bool() {
                entity.offset = rng.i64(-1..=boundary + 1);
            } else {
                entity.length = [i64::MAX, -1, 0, 1][rng.usize(..4)];
            }
        }
        let expected = entities.iter().enumerate().all(|(index, left)| {
            let Some(left_end) = left.offset.checked_add(left.length) else {
                return false;
            };
            left.length > 0
                && boundaries.contains(&left.offset)
                && boundaries.contains(&left_end)
                && entities[..index].iter().all(|right| {
                    // Earlier pairs have already passed the checked range test.
                    let right_end = right.offset + right.length;
                    left_end <= right.offset
                        || right_end <= left.offset
                        || (left.kind != right.kind
                            && ((left.offset <= right.offset && right_end <= left_end)
                                || (right.offset <= left.offset && left_end <= right_end)))
                })
        });
        let result = RichText::try_from_entities(&text, entities);
        assert_eq!(result.is_ok(), expected, "iteration {iteration}");
        if let Ok(rich) = result {
            let (text, entities) = rich.into_parts();
            let before = serde_json::to_vec(&entities).unwrap();
            let normalized = RichText::try_from_entities(text, entities).unwrap();
            assert_eq!(serde_json::to_vec(normalized.entities()).unwrap(), before);
        }
    }
}
