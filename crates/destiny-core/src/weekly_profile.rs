//! Bounded, provider-neutral character activity and milestone progress.
use crate::{AppError, CharacterId, DestinyMembership, ErrorCategory, Timestamp};

pub const MAX_WEEKLY_ACTIVITIES: usize = 512;
pub const MAX_WEEKLY_MILESTONES: usize = 64;
pub const MAX_WEEKLY_NESTED: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChallengeProgress {
    pub objective_hash: u32,
    pub progress: i64,
    pub completion_value: i64,
    pub complete: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableActivity {
    pub activity_hash: u32,
    pub focused: bool,
    /// Lifetime/provider completion (`isCompleted`), NOT weekly completion.
    pub completed_ever: bool,
    pub recommended_light: Option<i32>,
    pub difficulty_tier: Option<u32>,
    pub modifier_hashes: Vec<u32>,
    pub challenges: Vec<ChallengeProgress>,
    pub visible_reward_item_hashes: Vec<u32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestonePhase {
    pub phase_hash: u32,
    pub complete: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterMilestoneActivity {
    pub activity_hash: u32,
    pub challenges: Vec<ChallengeProgress>,
    pub phases: Vec<MilestonePhase>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneRewardEntry {
    pub entry_hash: u32,
    pub earned: bool,
    pub redeemed: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneReward {
    pub category_hash: u32,
    pub entries: Vec<MilestoneRewardEntry>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterMilestone {
    pub milestone_hash: u32,
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    pub activities: Vec<CharacterMilestoneActivity>,
    pub rewards: Vec<MilestoneReward>,
}
/// CharacterId's own Debug implementation keeps the opaque identifier redacted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterWeeklySnapshot {
    pub character_id: CharacterId,
    pub available_activities: Vec<AvailableActivity>,
    pub milestones: Vec<CharacterMilestone>,
}
impl CharacterWeeklySnapshot {
    /// Revalidates collection bounds and reset windows for constructed snapshots.
    pub fn validate(&self) -> Result<(), AppError> {
        let oversized = |length| length > MAX_WEEKLY_NESTED;
        if self.available_activities.len() > MAX_WEEKLY_ACTIVITIES
            || self.milestones.len() > MAX_WEEKLY_MILESTONES
            || self.available_activities.iter().any(|activity| {
                oversized(activity.modifier_hashes.len())
                    || oversized(activity.challenges.len())
                    || oversized(activity.visible_reward_item_hashes.len())
            })
            || self.milestones.iter().any(|milestone| {
                oversized(milestone.activities.len())
                    || oversized(milestone.rewards.len())
                    || matches!((milestone.start, milestone.end), (Some(start), Some(end)) if start > end)
                    || milestone.activities.iter().any(|activity| {
                        oversized(activity.challenges.len()) || oversized(activity.phases.len())
                    })
                    || milestone.rewards.iter().any(|reward| oversized(reward.entries.len()))
            })
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }
}
/// Separate read port: existing inventory gateway implementors remain unchanged.
pub trait WeeklyProfileGateway {
    fn load_character_weekly(
        &mut self,
        membership: &DestinyMembership,
        character: &CharacterId,
    ) -> Result<CharacterWeeklySnapshot, AppError>;
}
