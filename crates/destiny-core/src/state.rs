//! Provider-neutral freshness and repository revision policies.

use crate::domain::SnapshotMetadata;
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

/// A provider-neutral UTC instant used to order profile acquisitions, not TTL age.
/// Seconds are signed Unix seconds; nanoseconds preserve sub-second ordering.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProfileMintedAt {
    seconds: i64,
    nanoseconds: u32,
}

impl ProfileMintedAt {
    pub fn new(seconds: i64, nanoseconds: u32) -> Result<Self, crate::AppError> {
        if nanoseconds >= 1_000_000_000 {
            return Err(crate::AppError::new(
                crate::ErrorCategory::ProviderValidation,
            ));
        }
        Ok(Self {
            seconds,
            nanoseconds,
        })
    }

    pub const fn seconds(self) -> i64 {
        self.seconds
    }
    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
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

/// Why one profile synchronization was requested.
///
/// The reason is caller-supplied policy input, never provider data. `Ordinary`
/// may skip acquisition when the committed projection is already fresh; every
/// other reason refreshes regardless of freshness, because an explicit user
/// action, a mutation preflight, or a just-completed action must never read a
/// stale projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileSyncReason {
    Ordinary,
    Forced,
    Preflight,
    PostAction,
}

impl ProfileSyncReason {
    /// True when this reason must refresh even if the projection is fresh.
    pub const fn forces_refresh(self) -> bool {
        !matches!(self, Self::Ordinary)
    }

    /// True when this read is the observation half of a mutation execution.
    ///
    /// A preflight or post-action read must report a provider-unavailable
    /// response rather than silently absorb it: an unavailable observation must
    /// never be mistaken for a completed one, so these reasons are the only ones
    /// that require the conservative one-shot read scope. Every other reason
    /// (an ordinary or forced refresh) is a plain read that may repeat a
    /// transient failure under the bounded policy.
    pub const fn observes_mutation(self) -> bool {
        match self {
            Self::Preflight | Self::PostAction => true,
            Self::Ordinary | Self::Forced => false,
        }
    }
}

/// The result of one profile synchronization decision.
///
/// `SkippedFresh` means the projection was fresh and no acquisition happened.
/// `DiscardedNotNewer` means an Ordinary acquisition was not newer than the
/// retained profile; it is not a publication or new freshness evidence.
/// `Refreshed` means a complete candidate was acquired and committed. All carry the
/// [`SnapshotMetadata`] the repository reports, so revision, freshness, and
/// provenance stay repository-owned and this type invents no parallel metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileSyncOutcome {
    SkippedFresh { metadata: SnapshotMetadata },
    DiscardedNotNewer { metadata: SnapshotMetadata },
    Refreshed { metadata: SnapshotMetadata },
}

impl ProfileSyncOutcome {
    /// The committed metadata that describes the projection after this decision.
    pub fn metadata(&self) -> &SnapshotMetadata {
        match self {
            Self::SkippedFresh { metadata }
            | Self::DiscardedNotNewer { metadata }
            | Self::Refreshed { metadata } => metadata,
        }
    }

    /// The repository revision of the projection after this decision.
    pub fn revision(&self) -> SyncRevision {
        self.metadata().revision
    }

    /// True only when one complete candidate was acquired and committed.
    pub fn refreshed(&self) -> bool {
        match self {
            Self::SkippedFresh { .. } | Self::DiscardedNotNewer { .. } => false,
            Self::Refreshed { metadata: _ } => true,
        }
    }
}

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

    fn snapshot_metadata(revision: u64) -> SnapshotMetadata {
        use crate::ids::{DefinitionVersion, MembershipId, MembershipType};

        SnapshotMetadata {
            membership_id: MembershipId::new("4611686018484639825"),
            membership_type: MembershipType::new(3),
            revision: SyncRevision::new(revision),
            definition_version: DefinitionVersion::new("manifest-v1"),
            checked_at: Timestamp::new(100),
            source: SourceMarker::Profile,
        }
    }

    #[test]
    fn only_ordinary_sync_reason_may_skip_a_fresh_projection() {
        assert!(!ProfileSyncReason::Ordinary.forces_refresh());
        for reason in [
            ProfileSyncReason::Forced,
            ProfileSyncReason::Preflight,
            ProfileSyncReason::PostAction,
        ] {
            assert!(reason.forces_refresh(), "{reason:?} must force a refresh");
        }
    }

    #[test]
    fn sync_outcome_separates_skipped_from_refreshed_and_carries_repository_metadata() {
        let skipped = ProfileSyncOutcome::SkippedFresh {
            metadata: snapshot_metadata(4),
        };
        assert!(!skipped.refreshed());
        assert_eq!(skipped.revision(), SyncRevision::new(4));
        assert_eq!(skipped.metadata(), &snapshot_metadata(4));

        let refreshed = ProfileSyncOutcome::Refreshed {
            metadata: snapshot_metadata(5),
        };
        assert!(refreshed.refreshed());
        assert_eq!(refreshed.revision(), SyncRevision::new(5));
        assert_eq!(refreshed.metadata().source, SourceMarker::Profile);

        // Equal metadata never collapses the two terminal shapes.
        assert_ne!(
            ProfileSyncOutcome::SkippedFresh {
                metadata: snapshot_metadata(5)
            },
            ProfileSyncOutcome::Refreshed {
                metadata: snapshot_metadata(5)
            }
        );
    }
}
