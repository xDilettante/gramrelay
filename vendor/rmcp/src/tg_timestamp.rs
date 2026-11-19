//! UTC compatibility helpers for the downstream maintained-clock patch.

use jiff::{Timestamp, fmt::temporal::DateTimePrinter, tz::Offset};

pub(crate) fn rfc3339(timestamp: Timestamp) -> String {
    let nanos = timestamp.subsec_nanosecond();
    let precision = if nanos == 0 {
        0
    } else if nanos % 1_000_000 == 0 {
        3
    } else if nanos % 1_000 == 0 {
        6
    } else {
        9
    };
    let mut output = String::with_capacity(35);
    DateTimePrinter::new()
        .precision(Some(precision))
        .print_timestamp_with_offset(&timestamp, Offset::UTC, &mut output)
        .expect("formatting a timestamp into String cannot fail");
    output
}

#[cfg(any(feature = "request-state", test))]
pub(crate) fn epoch_millis(timestamp: Timestamp) -> i64 {
    // Preserve chrono's floor semantics before the epoch. Jiff's direct
    // as_millisecond() truncates toward zero. Its bounded timestamp range fits
    // i64 milliseconds (including both extrema), so this conversion is exact.
    i64::try_from(timestamp.as_nanosecond().div_euclid(1_000_000))
        .expect("Jiff's timestamp range fits i64 milliseconds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_auto_precision_golden_values_and_round_trips() {
        for (source, expected) in [
            ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00+00:00"),
            ("2000-01-01T00:00:00.1Z", "2000-01-01T00:00:00.100+00:00"),
            ("2000-01-01T00:00:00.001Z", "2000-01-01T00:00:00.001+00:00"),
            (
                "2000-01-01T00:00:00.0001Z",
                "2000-01-01T00:00:00.000100+00:00",
            ),
            (
                "2000-01-01T00:00:00.1234Z",
                "2000-01-01T00:00:00.123400+00:00",
            ),
            (
                "2000-01-01T00:00:00.000000001Z",
                "2000-01-01T00:00:00.000000001+00:00",
            ),
            (
                "2000-01-01T00:00:00.123456789Z",
                "2000-01-01T00:00:00.123456789+00:00",
            ),
            (
                "1969-12-31T23:59:59.999999999Z",
                "1969-12-31T23:59:59.999999999+00:00",
            ),
            ("1969-12-31T23:59:59.999Z", "1969-12-31T23:59:59.999+00:00"),
            (
                "2024-02-29T23:59:59.000001Z",
                "2024-02-29T23:59:59.000001+00:00",
            ),
            ("2000-01-01T00:30:00+01:00", "1999-12-31T23:30:00+00:00"),
            ("0000-01-01T00:00:00Z", "0000-01-01T00:00:00+00:00"),
            (
                "9999-12-30T00:00:00.999999999Z",
                "9999-12-30T00:00:00.999999999+00:00",
            ),
        ] {
            let timestamp = source
                .parse::<Timestamp>()
                .unwrap_or_else(|error| panic!("{source}: {error}"));
            let actual = rfc3339(timestamp);
            assert_eq!(actual, expected, "source: {source}");
            assert_eq!(actual.parse::<Timestamp>().unwrap(), timestamp);
        }
    }

    #[test]
    fn epoch_milliseconds_floor_negative_fractions_and_fit_extrema() {
        for (nanoseconds, milliseconds) in [
            (0, 0),
            (1, 0),
            (999_999, 0),
            (1_000_000, 1),
            (1_000_001, 1),
            (-1, -1),
            (-999_999, -1),
            (-1_000_000, -1),
            (-1_000_001, -2),
            (-999_999_999, -1_000),
            (-1_000_000_001, -1_001),
        ] {
            assert_eq!(
                epoch_millis(Timestamp::from_nanosecond(nanoseconds).unwrap()),
                milliseconds
            );
        }
        for timestamp in [Timestamp::MIN, Timestamp::MAX] {
            let milliseconds = epoch_millis(timestamp);
            let nanos = timestamp.as_nanosecond();
            assert!(i128::from(milliseconds) * 1_000_000 <= nanos);
            assert!(nanos < (i128::from(milliseconds) + 1) * 1_000_000);
        }
    }
}
