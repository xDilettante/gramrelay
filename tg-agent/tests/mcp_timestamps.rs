#![cfg(feature = "mcp")]

use rmcp::model::{Annotations, Timestamp};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn sdk_annotation_builders_preserve_utc_fraction_shape() {
    for (source, expected) in [
        ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00+00:00"),
        ("2024-02-29T00:00:00.1Z", "2024-02-29T00:00:00.100+00:00"),
        (
            "2000-01-01T03:00:00.1234+03:00",
            "2000-01-01T00:00:00.123400+00:00",
        ),
        (
            "1969-12-31T23:59:59.999999999Z",
            "1969-12-31T23:59:59.999999999+00:00",
        ),
    ] {
        let timestamp = source.parse::<Timestamp>().unwrap();
        let annotation = Annotations::for_resource(0.5, timestamp);
        assert_eq!(annotation.last_modified.as_deref(), Some(expected));
        assert_eq!(annotation.priority, Some(0.5));
        assert_eq!(
            Annotations::default()
                .with_timestamp(timestamp)
                .last_modified,
            annotation.last_modified
        );
        let wire = serde_json::to_value(&annotation).unwrap();
        assert_eq!(wire["lastModified"], expected);
        assert_eq!(
            serde_json::from_value::<Annotations>(wire).unwrap(),
            annotation
        );
    }
}

#[test]
fn sdk_annotation_raw_timestamps_are_opaque() {
    for raw in [
        "2025-01-12",
        "not-a-date",
        "2016-12-31T23:59:60Z",
        "2025-01-12T15:00:58.123400+03:00",
    ] {
        let annotation: Annotations = serde_json::from_value(json!({"lastModified":raw})).unwrap();
        assert_eq!(annotation.last_modified.as_deref(), Some(raw));
        assert_eq!(
            serde_json::to_value(annotation).unwrap(),
            json!({"lastModified":raw})
        );
    }
    for wire in [json!({}), json!({"lastModified": null})] {
        let annotation: Annotations = serde_json::from_value(wire).unwrap();
        assert_eq!(annotation.last_modified, None);
        assert_eq!(serde_json::to_value(annotation).unwrap(), json!({}));
    }
}

#[test]
fn sdk_task_and_annotation_clocks_use_current_utc_seconds() {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let annotation = Annotations::default()
        .with_timestamp_now()
        .last_modified
        .unwrap();
    let task = rmcp::task_manager::current_timestamp();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for value in [annotation, task] {
        assert!(value.ends_with("+00:00"));
        let timestamp = value.parse::<Timestamp>().unwrap();
        assert!((before..=after).contains(&timestamp.as_second()));
    }
}

#[test]
fn sdk_timeout_diagnostic_handles_the_full_std_duration_range() {
    for timeout in [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_millis(1234),
        Duration::MAX,
    ] {
        let error = rmcp::ServiceError::Timeout { timeout };
        assert_eq!(
            error.to_string(),
            format!("request timeout after {timeout:?}")
        );
    }
}
