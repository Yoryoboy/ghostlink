//! Pure weekly rotation predictions; no clock, date parsing, or provider access.

use crate::Timestamp;

pub const WEEK: u64 = 7 * 24 * 60 * 60;
/// Maximum UTF-8 byte length of calendar keys, names, titles, and coverage notes.
pub const MAX_CALENDAR_TEXT_LENGTH: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarError {
    InvalidText,
    EmptyRotations,
    DuplicateRotationKey,
    EmptyEntries,
    EmptySlot,
    AnchorOutOfRange,
    WindowOutOfRange,
}

fn validate_text(text: &str) -> Result<(), CalendarError> {
    if text.trim().is_empty() || text.len() > MAX_CALENDAR_TEXT_LENGTH {
        return Err(CalendarError::InvalidText);
    }
    Ok(())
}

/// Stable activity identity and display text, independent of the Manifest provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityReference {
    key: String,
    name: String,
    hash: u32,
}

impl ActivityReference {
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        hash: u32,
    ) -> Result<Self, CalendarError> {
        let key = key.into();
        let name = name.into();
        validate_text(&key)?;
        validate_text(&name)?;
        Ok(Self { key, name, hash })
    }

    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn hash(&self) -> u32 {
        self.hash
    }
}

/// Validated ordered slots; private fields preserve constructor invariants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rotation {
    key: String,
    title: String,
    anchor: usize,
    coverage_start: Timestamp,
    coverage_note: String,
    entries: Vec<Vec<ActivityReference>>,
}

impl Rotation {
    pub fn new(
        key: impl Into<String>,
        title: impl Into<String>,
        anchor: usize,
        coverage_start: Timestamp,
        coverage_note: impl Into<String>,
        entries: Vec<Vec<ActivityReference>>,
    ) -> Result<Self, CalendarError> {
        let key = key.into();
        let title = title.into();
        let coverage_note = coverage_note.into();
        validate_text(&key)?;
        validate_text(&title)?;
        validate_text(&coverage_note)?;
        if entries.is_empty() {
            return Err(CalendarError::EmptyEntries);
        }
        if entries.iter().any(Vec::is_empty) {
            return Err(CalendarError::EmptySlot);
        }
        if anchor >= entries.len() {
            return Err(CalendarError::AnchorOutOfRange);
        }
        Ok(Self {
            key,
            title,
            anchor,
            coverage_start,
            coverage_note,
            entries,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub const fn anchor(&self) -> usize {
        self.anchor
    }
    pub const fn coverage_start(&self) -> Timestamp {
        self.coverage_start
    }
    pub fn coverage_note(&self) -> &str {
        &self.coverage_note
    }
    pub fn entries(&self) -> &[Vec<ActivityReference>] {
        &self.entries
    }

    /// Euclidean modulo keeps pre-reference weeks inside the ordered slot list.
    pub fn slot_index(&self, week: i64) -> usize {
        (self.anchor as i128 + i128::from(week)).rem_euclid(self.entries.len() as i128) as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Calendar {
    reference_reset: Timestamp,
    rotations: Vec<Rotation>,
}

impl Calendar {
    pub fn new(
        reference_reset: Timestamp,
        rotations: Vec<Rotation>,
    ) -> Result<Self, CalendarError> {
        if rotations.is_empty() {
            return Err(CalendarError::EmptyRotations);
        }
        let mut keys = std::collections::BTreeSet::new();
        if rotations
            .iter()
            .any(|rotation| !keys.insert(rotation.key()))
        {
            return Err(CalendarError::DuplicateRotationKey);
        }
        Ok(Self {
            reference_reset,
            rotations,
        })
    }

    pub const fn reference_reset(&self) -> Timestamp {
        self.reference_reset
    }
    pub fn rotations(&self) -> &[Rotation] {
        &self.rotations
    }

    /// Evaluate the supplied instant. Callers own date and end-of-day semantics.
    pub fn week_at(&self, t: Timestamp) -> Result<CalendarWeek<'_>, CalendarError> {
        let index = week_index(self.reference_reset, t);
        let window = week_window(self.reference_reset, index)?;
        let rotations = self
            .rotations
            .iter()
            .map(|rotation| RotationPrediction {
                rotation,
                outcome: if window.start < rotation.coverage_start {
                    RotationOutcome::NoData {
                        coverage_start: rotation.coverage_start,
                        coverage_note: &rotation.coverage_note,
                    }
                } else {
                    RotationOutcome::Predicted(&rotation.entries[rotation.slot_index(index)])
                },
            })
            .collect();
        Ok(CalendarWeek {
            index,
            window,
            rotations,
        })
    }
}

/// Half-open weekly window: start is included, end belongs to the next week.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WeekWindow {
    pub start: Timestamp,
    pub end: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RotationOutcome<'a> {
    Predicted(&'a [ActivityReference]),
    NoData {
        coverage_start: Timestamp,
        coverage_note: &'a str,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationPrediction<'a> {
    pub rotation: &'a Rotation,
    pub outcome: RotationOutcome<'a>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarWeek<'a> {
    pub index: i64,
    pub window: WeekWindow,
    pub rotations: Vec<RotationPrediction<'a>>,
}

/// Signed floor division across the entire unsigned Timestamp range.
pub fn week_index(reference_reset: Timestamp, t: Timestamp) -> i64 {
    // A u64 second difference divided by WEEK always fits in i64.
    let difference = i128::from(t.seconds()) - i128::from(reference_reset.seconds());
    difference.div_euclid(i128::from(WEEK)) as i64
}

/// Reject windows whose start or exclusive end cannot be a Timestamp.
pub fn week_window(reference_reset: Timestamp, week: i64) -> Result<WeekWindow, CalendarError> {
    // Even i64 extreme weeks times WEEK fit in i128.
    let start = i128::from(reference_reset.seconds()) + i128::from(week) * i128::from(WEEK);
    let end = start + i128::from(WEEK);
    let start = u64::try_from(start).map_err(|_| CalendarError::WindowOutOfRange)?;
    let end = u64::try_from(end).map_err(|_| CalendarError::WindowOutOfRange)?;
    Ok(WeekWindow {
        start: Timestamp::new(start),
        end: Timestamp::new(end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix seconds: 2026-01-01T00:00Z = 1_767_225_600;
    // September 22 is day offset 264, June 16 is offset 166.
    const REFERENCE: Timestamp = Timestamp::new(1_790_096_400); // 2026-09-22T17:00Z
    const COVERAGE: Timestamp = Timestamp::new(1_781_629_200); // 2026-06-16T17:00Z

    fn rotation(key: &str, anchor: usize, pairs: &[[&str; 2]]) -> Rotation {
        let entries = pairs
            .iter()
            .map(|pair| {
                pair.iter()
                    .map(|key| ActivityReference::new(*key, *key, 1).unwrap())
                    .collect()
            })
            .collect();
        Rotation::new(key, key, anchor, COVERAGE, "Verified from June 16", entries).unwrap()
    }

    fn calendar() -> Calendar {
        let raid = rotation(
            "raid",
            6,
            &[
                ["vow-of-the-disciple", "last-wish"],
                ["kings-fall", "garden-of-salvation"],
                ["root-of-nightmares", "deep-stone-crypt"],
                ["crotas-end", "vault-of-glass"],
                ["salvations-edge", "vow-of-the-disciple"],
                ["kings-fall", "last-wish"],
                ["root-of-nightmares", "garden-of-salvation"],
                ["deep-stone-crypt", "crotas-end"],
                ["salvations-edge", "vault-of-glass"],
            ],
        );
        let pantheon = rotation(
            "pantheon",
            2,
            &[
                ["reprise-gahlran", "encore-consecrated-mind"],
                ["reprise-calus", "encore-morgeth"],
                ["reprise-argos", "encore-insurrection-prime"],
                ["reprise-gahlran", "encore-warpriest"],
                ["reprise-calus", "encore-consecrated-mind"],
                ["reprise-argos", "encore-morgeth"],
                ["reprise-gahlran", "encore-insurrection-prime"],
                ["reprise-calus", "encore-warpriest"],
                ["reprise-argos", "encore-consecrated-mind"],
                ["reprise-gahlran", "encore-morgeth"],
                ["reprise-calus", "encore-insurrection-prime"],
                ["reprise-argos", "encore-warpriest"],
            ],
        );
        Calendar::new(REFERENCE, vec![raid, pantheon]).unwrap()
    }

    fn keys<'a>(prediction: &'a RotationPrediction<'a>) -> Vec<&'a str> {
        match &prediction.outcome {
            RotationOutcome::Predicted(activities) => activities.iter().map(|a| a.key()).collect(),
            RotationOutcome::NoData { .. } => panic!("expected a covered week"),
        }
    }

    #[test]
    fn pinned_real_rotation_vectors() {
        let calendar = calendar();
        // Instants are 23:59:59 UTC on the commented days, not date parsing.
        let vectors = [
            (
                1_790_121_599,
                0,
                ["root-of-nightmares", "garden-of-salvation"],
                ["reprise-argos", "encore-insurrection-prime"],
            ), // 2026-09-22
            (
                1_790_726_399,
                1,
                ["deep-stone-crypt", "crotas-end"],
                ["reprise-gahlran", "encore-warpriest"],
            ), // 2026-09-29
            (
                1_791_331_199,
                2,
                ["salvations-edge", "vault-of-glass"],
                ["reprise-calus", "encore-consecrated-mind"],
            ), // 2026-10-06
            (
                1_791_935_999,
                3,
                ["vow-of-the-disciple", "last-wish"],
                ["reprise-argos", "encore-morgeth"],
            ), // 2026-10-13
            (
                1_781_654_399,
                -14,
                ["kings-fall", "garden-of-salvation"],
                ["reprise-gahlran", "encore-consecrated-mind"],
            ), // 2026-06-16
        ];
        for (seconds, index, raid, pantheon) in vectors {
            let week = calendar.week_at(Timestamp::new(seconds)).unwrap();
            assert_eq!(week.index, index);
            assert_eq!(week.rotations[0].rotation.key(), "raid");
            assert_eq!(week.rotations[1].rotation.key(), "pantheon");
            assert_eq!(keys(&week.rotations[0]), raid);
            assert_eq!(keys(&week.rotations[1]), pantheon);
            assert_eq!(
                week.window.end.seconds() - week.window.start.seconds(),
                WEEK
            );
            if index == -14 {
                assert_eq!(week.window.start, COVERAGE);
            }
        }
    }

    #[test]
    fn coverage_uses_week_start_and_is_per_rotation() {
        let mut rotations = calendar().rotations().to_vec();
        rotations.push(
            Rotation::new(
                "later",
                "Later",
                0,
                REFERENCE,
                "Later evidence",
                vec![vec![ActivityReference::new("a", "A", 42).unwrap()]],
            )
            .unwrap(),
        );
        let calendar = Calendar::new(REFERENCE, rotations).unwrap();
        let monday = calendar.week_at(Timestamp::new(1_781_567_999)).unwrap(); // 2026-06-15T23:59:59Z
        assert_eq!(monday.index, -15);
        assert_eq!(monday.window.start, Timestamp::new(1_781_024_400)); // 2026-06-09T17:00Z
        assert_eq!(
            monday.rotations[0].outcome,
            RotationOutcome::NoData {
                coverage_start: COVERAGE,
                coverage_note: "Verified from June 16"
            }
        );
        let june = calendar.week_at(COVERAGE).unwrap();
        assert!(matches!(
            june.rotations[0].outcome,
            RotationOutcome::Predicted(_)
        ));
        assert!(matches!(
            june.rotations[2].outcome,
            RotationOutcome::NoData { .. }
        ));
        assert!(matches!(
            calendar.week_at(REFERENCE).unwrap().rotations[2].outcome,
            RotationOutcome::Predicted(_)
        ));
    }

    #[test]
    fn reset_boundary_uses_signed_floor() {
        for index in [-14, -1, 0, 1] {
            let window = week_window(REFERENCE, index).unwrap();
            assert_eq!(week_index(REFERENCE, window.start), index);
            assert_eq!(
                week_index(REFERENCE, Timestamp::new(window.start.seconds() - 1)),
                index - 1
            );
            assert_eq!(week_index(REFERENCE, window.end), index + 1);
        }
    }

    #[test]
    fn timestamp_extremes_are_checked_without_overflow() {
        let zero = Timestamp::new(0);
        let max = Timestamp::new(u64::MAX);
        assert_eq!(
            week_index(zero, max),
            (i128::from(u64::MAX) / i128::from(WEEK)) as i64
        );
        assert_eq!(
            week_index(max, zero),
            (-i128::from(u64::MAX)).div_euclid(i128::from(WEEK)) as i64
        );
        for (reference, index) in [
            (zero, -1),
            (max, 0),
            (REFERENCE, i64::MIN),
            (REFERENCE, i64::MAX),
        ] {
            assert_eq!(
                week_window(reference, index),
                Err(CalendarError::WindowOutOfRange)
            );
        }
        assert_eq!(week_window(zero, 0).unwrap().start, zero);
        assert_eq!(
            week_window(Timestamp::new(u64::MAX - WEEK), 0).unwrap().end,
            max
        );
        assert_eq!(
            calendar().week_at(zero),
            Err(CalendarError::WindowOutOfRange)
        );
    }

    #[test]
    fn constructors_reject_invalid_shapes() {
        assert_eq!(
            Calendar::new(REFERENCE, vec![]),
            Err(CalendarError::EmptyRotations)
        );
        let raid = calendar().rotations()[0].clone();
        assert_eq!(
            Calendar::new(REFERENCE, vec![raid.clone(), raid]),
            Err(CalendarError::DuplicateRotationKey)
        );
        let activity = ActivityReference::new("a", "A", 1).unwrap();
        assert_eq!(
            Rotation::new("r", "R", 0, COVERAGE, "Note", vec![]),
            Err(CalendarError::EmptyEntries)
        );
        assert_eq!(
            Rotation::new("r", "R", 0, COVERAGE, "Note", vec![vec![]]),
            Err(CalendarError::EmptySlot)
        );
        assert_eq!(
            Rotation::new("r", "R", 1, COVERAGE, "Note", vec![vec![activity]]),
            Err(CalendarError::AnchorOutOfRange)
        );
    }

    #[test]
    fn strings_are_nonblank_and_byte_bounded() {
        for invalid in [
            "".to_owned(),
            " \t".to_owned(),
            "x".repeat(MAX_CALENDAR_TEXT_LENGTH + 1),
        ] {
            assert_eq!(
                ActivityReference::new(&invalid, "A", 1),
                Err(CalendarError::InvalidText)
            );
            assert_eq!(
                ActivityReference::new("a", &invalid, 1),
                Err(CalendarError::InvalidText)
            );
            for (key, title, note) in [
                (&invalid[..], "Title", "Note"),
                ("key", &invalid[..], "Note"),
                ("key", "Title", &invalid[..]),
            ] {
                assert_eq!(
                    Rotation::new(key, title, 0, COVERAGE, note, vec![]),
                    Err(CalendarError::InvalidText)
                );
            }
        }
        let name = "x".repeat(MAX_CALENDAR_TEXT_LENGTH);
        let activity = ActivityReference::new("a", &name, u32::MAX).unwrap();
        assert_eq!(activity.name(), name);
        assert_eq!(activity.hash(), u32::MAX);
        assert!(ActivityReference::new("a", "é".repeat(MAX_CALENDAR_TEXT_LENGTH), 0).is_err());
    }
}
