use grammers_session::types::DcOption;
use serde_json::{Value, json};

fn wire() -> Value {
    json!({"id":2,"ipv4":"149.154.167.50:443","ipv6":"[2001:67c:4e8:f002::a]:443"})
}

#[test]
fn dc_key_wire_compatibility_covers_all_byte_values_and_mixed_case() {
    let key = std::array::from_fn::<_, 256, _>(|index| index as u8);
    let lower: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut source = wire();
    source["auth_key"] = json!(lower);
    let option: DcOption = serde_json::from_value(source.clone()).unwrap();
    assert_eq!(option.auth_key, Some(key));
    assert_eq!(serde_json::to_value(&option).unwrap(), source);
    let mixed: String = lower
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect();
    source["auth_key"] = json!(mixed);
    let option: DcOption = serde_json::from_value(source).unwrap();
    assert_eq!(option.auth_key, Some(key));
    assert_eq!(serde_json::to_value(option).unwrap()["auth_key"], lower);
}

#[test]
fn dc_key_null_missing_and_invalid_lengths_preserve_schema() {
    let mut source = wire();
    let missing: DcOption = serde_json::from_value(source.clone()).unwrap();
    assert_eq!(missing.auth_key, None);
    source["auth_key"] = Value::Null;
    assert_eq!(serde_json::to_value(missing).unwrap(), source);
    assert_eq!(
        serde_json::from_value::<DcOption>(source.clone())
            .unwrap()
            .auth_key,
        None
    );
    for key in [
        json!(""),
        json!("ab".repeat(255)),
        json!("ab".repeat(257)),
        json!("a".repeat(511)),
        json!("gg".repeat(256)),
        json!([0, 1]),
        json!(42),
    ] {
        source["auth_key"] = key;
        assert!(serde_json::from_value::<DcOption>(source.clone()).is_err());
    }
}

#[test]
fn malformed_key_strings_do_not_enter_error_diagnostics() {
    for raw in [
        "PRIVATE".to_owned(),
        "a".repeat(511),
        format!("{}PRIVATE!", "a".repeat(504)),
    ] {
        let mut source = wire();
        source["auth_key"] = json!(raw);
        let error = serde_json::from_value::<DcOption>(source).unwrap_err();
        assert!(error.to_string().contains("invalid authentication key"));
        assert!(!error.to_string().contains(&raw));
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
}

#[test]
fn malformed_key_type_does_not_enter_error_diagnostics() {
    let sentinel = 123456789;
    let mut source = wire();
    source["auth_key"] = json!(sentinel);
    let error = serde_json::from_value::<DcOption>(source).unwrap_err();
    assert!(error.to_string().contains("invalid authentication key"));
    assert!(!error.to_string().contains(&sentinel.to_string()));
}
