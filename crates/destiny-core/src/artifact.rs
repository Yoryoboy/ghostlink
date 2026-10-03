//! Live-only, provider-neutral seasonal artifact state.
use crate::{AppError, CharacterId, DestinyMembership, ErrorCategory, ItemHash};
use std::collections::HashSet;

pub const MAX_ARTIFACT_CHARACTERS: usize = 8;
pub const MAX_ARTIFACT_TIERS: usize = 16;
pub const MAX_ARTIFACT_PERKS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSnapshot {
    pub profile: Option<ArtifactProfileState>,
    pub characters: Vec<ArtifactCharacterState>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactProfileState {
    pub artifact_hash: ItemHash,
    pub points_acquired: u32,
    pub power_bonus: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactCharacterState {
    pub character_id: CharacterId,
    pub artifact_hash: ItemHash,
    pub points_used: u32,
    pub reset_count: u32,
    pub tiers: Vec<ArtifactTier>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactTier {
    pub tier_hash: u32,
    pub is_unlocked: bool,
    pub points_to_unlock: u32,
    pub perks: Vec<ArtifactPerk>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPerk {
    pub item_hash: ItemHash,
    pub active: bool,
}
impl ArtifactSnapshot {
    /// Revalidates bounds and uniqueness for constructed snapshots.
    pub fn validate(&self) -> Result<(), AppError> {
        let mut seen = HashSet::new();
        if self.characters.len() > MAX_ARTIFACT_CHARACTERS
            || self.characters.iter().any(|c| {
                !seen.insert(&c.character_id)
                    || c.tiers.len() > MAX_ARTIFACT_TIERS
                    || c.tiers.iter().any(|t| t.perks.len() > MAX_ARTIFACT_PERKS)
            })
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }
}
/// Separate live read port; no persistence or inventory gateway changes.
pub trait ArtifactGateway {
    fn load_artifact(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<ArtifactSnapshot, AppError>;
}
