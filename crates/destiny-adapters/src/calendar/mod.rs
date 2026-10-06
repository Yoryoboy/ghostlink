//! Embedded weekly rotation calendar loading.

use destiny_core::{
    Timestamp,
    calendar::{ActivityReference, Calendar, CalendarError, Rotation},
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub enum CalendarLoadError {
    Parse,
    InvalidInstant { field: String },
    UnknownActivity { key: String },
    UnusedActivity { key: String },
    UnknownRotation { key: String },
    InvalidSlotShape { rotation: String, slot: usize },
    Validation(CalendarError),
}

impl std::fmt::Display for CalendarLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse => f.write_str("invalid calendar TOML"),
            Self::InvalidInstant { field } => write!(f, "invalid calendar instant: {field}"),
            Self::UnknownActivity { key } => write!(f, "undefined calendar activity: {key}"),
            Self::UnusedActivity { key } => write!(f, "unused calendar activity: {key}"),
            Self::UnknownRotation { key } => write!(f, "unknown calendar rotation: {key}"),
            Self::InvalidSlotShape { rotation, slot } => {
                write!(f, "invalid calendar slot shape: {rotation}[{slot}]")
            }
            Self::Validation(error) => write!(f, "invalid calendar model: {error:?}"),
        }
    }
}

impl std::error::Error for CalendarLoadError {}

/// Load and validate the calendar compiled into this binary; no runtime I/O.
pub fn embedded_calendar() -> Result<Calendar, CalendarLoadError> {
    parse_calendar(include_str!("rotations.toml"))
}

/// UTC entry point for consumers that do not depend on chrono directly.
pub fn instant_for_utc_date(value: &str) -> Result<Timestamp, CalendarLoadError> {
    instant_for_date(value, &chrono::Utc)
}

/// Strict ISO calendar date, evaluated at the end of the day in `zone`.
/// Pass `Utc` for UTC semantics. Ambiguous times select the later instant;
/// gaps advance to the next valid local second (bounded to two days).
/// Core timestamps cannot represent dates before the Unix epoch.
pub fn instant_for_date<Tz: chrono::TimeZone>(
    value: &str,
    zone: &Tz,
) -> Result<Timestamp, CalendarLoadError> {
    let invalid = || CalendarLoadError::InvalidInstant {
        field: "date".into(),
    };
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid())?;
    let mut local = date.and_hms_opt(23, 59, 59).ok_or_else(invalid)?;
    for _ in 0..=2 * 24 * 60 * 60 {
        let instant = match zone.from_local_datetime(&local) {
            chrono::LocalResult::Single(instant) => Some(instant),
            chrono::LocalResult::Ambiguous(a, b) => {
                Some(if a.timestamp() > b.timestamp() { a } else { b })
            }
            chrono::LocalResult::None => None,
        };
        if let Some(instant) = instant {
            return Ok(Timestamp::new(
                u64::try_from(instant.timestamp()).map_err(|_| invalid())?,
            ));
        }
        local = local
            .checked_add_signed(chrono::Duration::seconds(1))
            .ok_or_else(invalid)?;
    }
    Err(invalid())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    reference_reset: String,
    activities: BTreeMap<String, Activity>,
    rotations: BTreeMap<String, RotationRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activity {
    name: String,
    hash: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationRecord {
    title: String,
    anchor: usize,
    coverage_start: String,
    coverage_note: String,
    entries: Vec<Vec<String>>,
}

fn parse_instant(value: &str, field: &str) -> Result<Timestamp, CalendarLoadError> {
    let invalid = || CalendarLoadError::InvalidInstant {
        field: field.into(),
    };
    let date = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| invalid())?;
    // Core timestamps represent whole nonnegative Unix seconds. Do not silently
    // truncate fractional seconds or chrono's leap-second representation.
    if date.timestamp_subsec_nanos() != 0 {
        return Err(invalid());
    }
    let seconds = u64::try_from(date.timestamp()).map_err(|_| invalid())?;
    Ok(Timestamp::new(seconds))
}

fn parse_calendar(text: &str) -> Result<Calendar, CalendarLoadError> {
    let document: Document = toml::from_str(text).map_err(|_| CalendarLoadError::Parse)?;
    let reference = parse_instant(&document.reference_reset, "reference_reset")?;
    let mut activities = BTreeMap::new();
    for (key, activity) in document.activities {
        let reference = ActivityReference::new(&key, activity.name, activity.hash)
            .map_err(CalendarLoadError::Validation)?;
        activities.insert(key, reference);
    }
    // TOML's default maps sort keys, not source order. Explicit canonical order
    // matches rotations.toml without adding an order-preserving dependency.
    const ORDER: [&str; 4] = ["raid", "dungeon", "pantheon", "grandmaster"];
    let mut records = document.rotations;
    if let Some(key) = records.keys().find(|key| !ORDER.contains(&key.as_str())) {
        return Err(CalendarLoadError::UnknownRotation { key: key.clone() });
    }
    let mut used = BTreeSet::new();
    let mut rotations = Vec::new();
    for key in ORDER {
        let Some(record) = records.remove(key) else {
            continue;
        };
        let coverage = parse_instant(
            &record.coverage_start,
            &format!("rotations.{key}.coverage_start"),
        )?;
        let entries = record
            .entries
            .into_iter()
            .map(|slot| {
                slot.into_iter()
                    .map(|activity_key| {
                        let activity = activities.get(&activity_key).ok_or_else(|| {
                            CalendarLoadError::UnknownActivity {
                                key: activity_key.clone(),
                            }
                        })?;
                        used.insert(activity_key);
                        Ok(activity.clone())
                    })
                    .collect::<Result<Vec<_>, CalendarLoadError>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rotation = Rotation::new(
            key,
            record.title,
            record.anchor,
            coverage,
            record.coverage_note,
            entries,
        )
        .map_err(CalendarLoadError::Validation)?;
        let width = if key == "grandmaster" { 1 } else { 2 };
        if let Some(slot) = rotation
            .entries()
            .iter()
            .position(|slot| slot.len() != width)
        {
            return Err(CalendarLoadError::InvalidSlotShape {
                rotation: key.into(),
                slot,
            });
        }
        rotations.push(rotation);
    }
    let calendar = Calendar::new(reference, rotations).map_err(CalendarLoadError::Validation)?;
    if let Some(key) = activities.keys().find(|key| !used.contains(*key)) {
        return Err(CalendarLoadError::UnusedActivity { key: key.clone() });
    }
    Ok(calendar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{Timestamp, calendar::RotationOutcome};
    use std::collections::BTreeMap;

    #[test]
    fn strict_calendar_date_uses_end_of_utc_day() {
        assert_eq!(
            instant_for_utc_date("2026-10-06").unwrap(),
            instant("2026-10-06T23:59:59Z")
        );
        for invalid in [
            "2026-13-01",
            "2026-10-6",
            "06/10/2026",
            "",
            "2026-02-29",
            "2026-10-06T00:00:00Z",
            " 2026-10-06",
            "1969-12-31",
        ] {
            assert!(
                instant_for_date(invalid, &chrono::Utc).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn calendar_date_uses_the_requested_zone() {
        let calendar = embedded_calendar().unwrap();
        for (date, offset, instant_text, week_start) in [
            (
                "2026-10-06",
                -3 * 3600,
                "2026-10-07T02:59:59Z",
                "2026-10-06T17:00:00Z",
            ),
            (
                "2026-10-06",
                10 * 3600,
                "2026-10-06T13:59:59Z",
                "2026-09-29T17:00:00Z",
            ),
            (
                "2026-10-07",
                10 * 3600,
                "2026-10-07T13:59:59Z",
                "2026-10-06T17:00:00Z",
            ),
            (
                "2026-10-06",
                0,
                "2026-10-06T23:59:59Z",
                "2026-10-06T17:00:00Z",
            ),
        ] {
            let zone = chrono::FixedOffset::east_opt(offset).unwrap();
            let timestamp = instant_for_date(date, &zone).unwrap();
            assert_eq!(timestamp, instant(instant_text));
            assert_eq!(
                calendar.week_at(timestamp).unwrap().window.start,
                instant(week_start)
            );
        }
    }

    #[derive(Clone)]
    struct TransitionZone(u8);
    impl chrono::TimeZone for TransitionZone {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &Self::Offset) -> Self {
            Self(0)
        }
        fn offset_from_local_date(
            &self,
            date: &chrono::NaiveDate,
        ) -> chrono::LocalResult<Self::Offset> {
            self.offset_from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::LocalResult<Self::Offset> {
            let one = chrono::FixedOffset::east_opt(3600).unwrap();
            match self.0 {
                0 => chrono::LocalResult::Ambiguous(
                    one,
                    chrono::FixedOffset::east_opt(7200).unwrap(),
                ),
                1 if local.time() == chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap() => {
                    chrono::LocalResult::None
                }
                2 => chrono::LocalResult::None,
                _ => chrono::LocalResult::Single(one),
            }
        }
        fn offset_from_utc_date(&self, _: &chrono::NaiveDate) -> Self::Offset {
            chrono::FixedOffset::east_opt(3600).unwrap()
        }
        fn offset_from_utc_datetime(&self, _: &chrono::NaiveDateTime) -> Self::Offset {
            chrono::FixedOffset::east_opt(3600).unwrap()
        }
    }

    #[test]
    fn calendar_date_resolves_ambiguities_gaps_and_unresolvable_zones() {
        assert_eq!(
            instant_for_date("2026-10-06", &TransitionZone(0)).unwrap(),
            instant("2026-10-06T22:59:59Z")
        );
        assert_eq!(
            instant_for_date("2026-10-06", &TransitionZone(1)).unwrap(),
            instant("2026-10-06T23:00:00Z")
        );
        assert!(instant_for_date("2026-10-06", &TransitionZone(2)).is_err());
    }

    const FIXTURE: &str = r#"
reference_reset = "2026-09-22T17:00:00Z"
[activities.alpha]
name = "Alpha"
hash = 42
[activities.beta]
name = "Beta"
hash = 43
[rotations.raid]
title = "Raids"
anchor = 0
coverage_start = "2026-06-16T17:00:00Z"
coverage_note = "Verified"
entries = [["alpha", "beta"]]
"#;

    #[test]
    fn calendar_inline_happy_path() {
        let calendar = parse_calendar(FIXTURE).unwrap();
        assert_eq!(calendar.rotations()[0].entries()[0][0].hash(), 42);
        assert_eq!(calendar.rotations()[0].entries()[0][1].name(), "Beta");
    }

    #[test]
    fn calendar_unknown_fields_rejected_at_every_level() {
        for fixture in [
            format!("unexpected = 1\n{FIXTURE}"),
            FIXTURE.replace("hash = 42", "hash = 42\nunexpected = 1"),
            format!("{FIXTURE}\nunexpected = 1"),
        ] {
            assert!(matches!(
                parse_calendar(&fixture),
                Err(CalendarLoadError::Parse)
            ));
        }
    }

    #[test]
    fn calendar_undefined_activity() {
        let fixture = FIXTURE.replace(
            "entries = [[\"alpha\", \"beta\"]]",
            "entries = [[\"alpha\", \"missing\"]]",
        );
        assert!(
            matches!(parse_calendar(&fixture), Err(CalendarLoadError::UnknownActivity { key }) if key == "missing")
        );
    }

    #[test]
    fn calendar_unused_activity() {
        let fixture = format!("{FIXTURE}\n[activities.unused]\nname = \"Unused\"\nhash = 44\n");
        assert!(
            matches!(parse_calendar(&fixture), Err(CalendarLoadError::UnusedActivity { key }) if key == "unused")
        );
    }

    #[test]
    fn calendar_bad_instants() {
        for instant in ["invalid", "1969-12-31T23:59:59Z", "2026-09-22T17:00:00.5Z"] {
            for original in ["2026-09-22T17:00:00Z", "2026-06-16T17:00:00Z"] {
                assert!(matches!(
                    parse_calendar(&FIXTURE.replace(original, instant)),
                    Err(CalendarLoadError::InvalidInstant { .. })
                ));
            }
        }
    }

    #[test]
    fn calendar_core_validation() {
        for (old, new, expected) in [
            ("anchor = 0", "anchor = 1", CalendarError::AnchorOutOfRange),
            (
                "entries = [[\"alpha\", \"beta\"]]",
                "entries = []",
                CalendarError::EmptyEntries,
            ),
            (
                "entries = [[\"alpha\", \"beta\"]]",
                "entries = [[]]",
                CalendarError::EmptySlot,
            ),
        ] {
            assert!(
                matches!(parse_calendar(&FIXTURE.replace(old, new)), Err(CalendarLoadError::Validation(error)) if error == expected)
            );
        }
    }

    #[test]
    fn calendar_wrong_shape_and_unknown_rotation() {
        assert!(matches!(
            parse_calendar(&FIXTURE.replace(
                "entries = [[\"alpha\", \"beta\"]]",
                "entries = [[\"alpha\"]]"
            )),
            Err(CalendarLoadError::InvalidSlotShape { .. })
        ));
        assert!(matches!(
            parse_calendar(&FIXTURE.replace("rotations.raid", "rotations.typo")),
            Err(CalendarLoadError::UnknownRotation { .. })
        ));
    }

    #[test]
    fn calendar_embedded_order_balance_and_shape() {
        let calendar = embedded_calendar().unwrap();
        assert_eq!(
            calendar
                .rotations()
                .iter()
                .map(|r| r.key())
                .collect::<Vec<_>>(),
            ["raid", "dungeon", "pantheon", "grandmaster"]
        );
        for rotation in calendar.rotations() {
            let width = if rotation.key() == "grandmaster" {
                1
            } else {
                2
            };
            assert!(rotation.entries().iter().all(|slot| slot.len() == width));
            if matches!(rotation.key(), "raid" | "dungeon") {
                let mut counts = BTreeMap::new();
                for activity in rotation.entries().iter().flatten() {
                    *counts.entry(activity.key()).or_insert(0) += 1;
                }
                assert!(counts.values().all(|count| *count == 2), "{counts:?}");
            }
        }
    }

    fn instant(value: &str) -> Timestamp {
        Timestamp::new(
            chrono::DateTime::parse_from_rfc3339(value)
                .unwrap()
                .timestamp()
                .try_into()
                .unwrap(),
        )
    }

    #[test]
    fn calendar_embedded_pinned_predictions() {
        let calendar = embedded_calendar().unwrap();
        let week = calendar.week_at(instant("2026-10-06T23:59:59Z")).unwrap();
        let expected: &[&[&str]] = &[
            &["salvations-edge", "vault-of-glass"],
            &["pit-of-heresy", "warlords-ruin"],
            &["reprise-calus", "encore-consecrated-mind"],
            &["the-arms-dealer"],
        ];
        for (prediction, keys) in week.rotations.iter().zip(expected) {
            let RotationOutcome::Predicted(activities) = &prediction.outcome else {
                panic!("expected prediction")
            };
            assert_eq!(
                activities.iter().map(|a| a.key()).collect::<Vec<_>>(),
                *keys
            );
        }
        let week = calendar.week_at(instant("2026-09-22T23:59:59Z")).unwrap();
        let RotationOutcome::Predicted(activities) = &week.rotations[1].outcome else {
            panic!("expected dungeon prediction")
        };
        assert_eq!(
            activities.iter().map(|a| a.key()).collect::<Vec<_>>(),
            ["sundered-doctrine", "spire-of-the-watcher"]
        );
        let week = calendar.week_at(instant("2026-06-15T23:59:59Z")).unwrap();
        assert!(
            week.rotations
                .iter()
                .all(|p| matches!(p.outcome, RotationOutcome::NoData { .. }))
        );
    }
}
