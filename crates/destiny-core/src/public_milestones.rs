//! Bounded, provider-neutral public weekly milestone projection.
use crate::{AppError, ErrorCategory, Timestamp};
use std::collections::HashSet;

/// A public activity and its ordered modifier hashes. No account data is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicMilestoneActivity {
    pub activity_hash: u32,
    pub modifier_hashes: Vec<u32>,
    pub challenge_objective_hashes: Vec<u32>,
}
/// One milestone's public reset window and activities, measured in UTC Unix seconds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicMilestone {
    pub milestone_hash: u32,
    pub order: u32,
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    pub activities: Vec<PublicMilestoneActivity>,
}
/// A complete public snapshot; consumers can revalidate constructed projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicMilestonesSnapshot {
    pub milestones: Vec<PublicMilestone>,
}
impl PublicMilestonesSnapshot {
    /// Rejects duplicates, oversized collections, and inverted reset windows.
    pub fn validate(&self) -> Result<(), AppError> {
        let mut seen = HashSet::new();
        if self.milestones.len() > 64 || self.milestones.iter().any(|milestone| {
            !seen.insert(milestone.milestone_hash)
                || milestone.activities.len() > 64
                || milestone.activities.iter().any(|activity| activity.modifier_hashes.len() > 64 || activity.challenge_objective_hashes.len() > 64)
                || matches!((milestone.start, milestone.end), (Some(start), Some(end)) if start > end)
        }) {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }
}
