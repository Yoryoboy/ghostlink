//! Provider-neutral freshness and repository revision policies.

use crate::ids::SyncRevision;

/// A deterministic, injected-clock timestamp measured in seconds from an application epoch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const fn new(seconds: u64) -> Self {
        Self(seconds)
    }
    pub const fn seconds(self) -> u64 {
        self.0
    }
}

/// Explicit freshness policy; providers do not choose a TTL in the core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FreshnessPolicy {
    pub ttl_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dataset {
    ManifestDefinitions,
    ProfileInventory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceMarker {
    Manifest,
    Profile,
}

impl Dataset {
    /// The single source marker permitted to commit this dataset.
    ///
    /// Freshness checks and durable metadata validation share this pairing so a
    /// dataset can never be committed under a mismatched provenance marker.
    pub const fn expected_source(self) -> SourceMarker {
        match self {
            Dataset::ManifestDefinitions => SourceMarker::Manifest,
            Dataset::ProfileInventory => SourceMarker::Profile,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshnessStatus {
    Missing,
    Fresh,
    Stale,
    InvalidSource,
}

/// The markers needed to evaluate one dataset without consulting a clock or provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FreshnessInput {
    pub dataset: Dataset,
    pub checked_at: Option<Timestamp>,
    pub source: Option<SourceMarker>,
    pub now: Timestamp,
    pub policy: FreshnessPolicy,
}

impl FreshnessInput {
    pub fn status(self) -> FreshnessStatus {
        let Some(checked_at) = self.checked_at else {
            return FreshnessStatus::Missing;
        };
        if self.source != Some(self.dataset.expected_source()) {
            return FreshnessStatus::InvalidSource;
        }
        if checked_at.seconds() > self.now.seconds() {
            return FreshnessStatus::Stale;
        }
        let age = self.now.seconds() - checked_at.seconds();
        if age <= self.policy.ttl_seconds {
            FreshnessStatus::Fresh
        } else {
            FreshnessStatus::Stale
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionState {
    pub revision: SyncRevision,
    pub normalized_fingerprint: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionAllocation {
    pub revision: SyncRevision,
    pub retained: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionOverflow;

/// Allocates repository revisions from normalized content identity only.
///
/// This compatibility helper predates durable metadata and remains exported for
/// callers that still rely on it. Repository revision retention must compare the
/// normalized projection content structurally and must not use a fingerprint
/// here, because distinct normalized inputs can collide on a single `u64`.
/// Freshness and provenance metadata (`checked_at`, `source`) are not content
/// identity and must not participate in retention.
pub fn allocate_revision(
    previous: Option<RevisionState>,
    normalized_fingerprint: u64,
) -> Result<RevisionAllocation, RevisionOverflow> {
    match previous {
        None => Ok(RevisionAllocation {
            revision: SyncRevision::new(1),
            retained: false,
        }),
        Some(previous) if previous.normalized_fingerprint == normalized_fingerprint => {
            Ok(RevisionAllocation {
                revision: previous.revision,
                retained: true,
            })
        }
        Some(previous) => {
            let current = previous.revision;
            let next = current.value().checked_add(1).ok_or(RevisionOverflow)?;
            Ok(RevisionAllocation {
                revision: SyncRevision::new(next),
                retained: false,
            })
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution<T> {
    Missing,
    Unique(T),
    Ambiguous(Vec<T>),
}

pub type SemanticResolution<T> = Resolution<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_boundary_is_fresh_until_ttl_is_exceeded() {
        let input = FreshnessInput {
            dataset: Dataset::ManifestDefinitions,
            checked_at: Some(Timestamp::new(10)),
            source: Some(SourceMarker::Manifest),
            now: Timestamp::new(20),
            policy: FreshnessPolicy { ttl_seconds: 10 },
        };
        assert_eq!(input.status(), FreshnessStatus::Fresh);
        assert_eq!(
            FreshnessInput {
                now: Timestamp::new(9),
                ..input
            }
            .status(),
            FreshnessStatus::Stale
        );
        assert_eq!(
            FreshnessInput {
                now: Timestamp::new(21),
                ..input
            }
            .status(),
            FreshnessStatus::Stale
        );
        assert_eq!(
            FreshnessInput {
                checked_at: None,
                ..input
            }
            .status(),
            FreshnessStatus::Missing
        );
    }

    #[test]
    fn freshness_rejects_mismatched_source_without_details() {
        let input = FreshnessInput {
            dataset: Dataset::ManifestDefinitions,
            checked_at: Some(Timestamp::new(10)),
            source: Some(SourceMarker::Profile),
            now: Timestamp::new(10),
            policy: FreshnessPolicy { ttl_seconds: 10 },
        };
        assert_eq!(input.status(), FreshnessStatus::InvalidSource);
    }

    #[test]
    fn revisions_retain_advance_and_fail_on_overflow() {
        let first = allocate_revision(None, 7).unwrap();
        assert_eq!(first.revision.value(), 1);
        let same = allocate_revision(
            Some(RevisionState {
                revision: first.revision,
                normalized_fingerprint: 7,
            }),
            7,
        )
        .unwrap();
        assert!(same.retained);
        let changed = allocate_revision(
            Some(RevisionState {
                revision: same.revision,
                normalized_fingerprint: 7,
            }),
            8,
        )
        .unwrap();
        assert_eq!(changed.revision.value(), 2);
        assert_eq!(
            allocate_revision(
                Some(RevisionState {
                    revision: SyncRevision::new(u64::MAX),
                    normalized_fingerprint: 7
                }),
                8
            ),
            Err(RevisionOverflow)
        );
    }

    #[test]
    fn resolution_distinguishes_missing_unique_and_ambiguous() {
        assert_eq!(Resolution::<u8>::Missing, Resolution::Missing);
        assert_eq!(Resolution::Unique(1), Resolution::Unique(1));
        assert_eq!(
            Resolution::Ambiguous(vec![1, 2]),
            Resolution::Ambiguous(vec![1, 2])
        );
    }
}
