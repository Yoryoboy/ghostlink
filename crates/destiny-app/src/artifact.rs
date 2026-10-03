//! Live seasonal artifact query over freshness-synchronized characters.
use crate::ProfileSyncService;
use destiny_core::{
    AppError, ArtifactGateway, CharacterId, DefinitionRepository, DestinyMembership, ErrorCategory,
    FreshnessPolicy, InventoryRepository, ItemHash, ProfileGateway, ProfileSyncReason, Resolution,
    Timestamp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactResult {
    pub artifact: Option<ArtifactSummary>,
    pub characters: Vec<CharacterArtifact>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSummary {
    pub artifact_hash: ItemHash,
    pub name: Option<String>,
    pub points_acquired: u32,
    pub power_bonus: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterArtifact {
    pub character_id: CharacterId,
    pub class: Option<String>,
    /// None means the live response omitted this character's artifact state.
    /// Some with empty tiers means state was present but had no visible perks.
    pub state: Option<CharacterArtifactState>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterArtifactState {
    pub points_used: u32,
    pub reset_count: u32,
    pub active_perks: Vec<ArtifactPerkView>,
    pub tiers: Vec<ArtifactTierView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactTierView {
    /// One-based position in provider tier order.
    pub index: usize,
    pub unlocked: bool,
    pub points_to_unlock: u32,
    pub perks: Vec<ArtifactPerkView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPerkView {
    pub item_hash: ItemHash,
    pub name: Option<String>,
    pub active: bool,
}

fn resolve_name(
    repository: &impl DefinitionRepository,
    hash: ItemHash,
) -> Result<Option<String>, AppError> {
    match repository.lookup_definition(hash)? {
        Resolution::Unique(definition) => Ok(Some(definition.name)),
        Resolution::Missing => Ok(None),
        Resolution::Ambiguous(_) => Err(AppError::new(ErrorCategory::LocalStorage)),
    }
}

/// Sibling read contract, like the separately composed weekly service.
pub trait ArtifactOperations {
    fn artifact(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ArtifactResult, AppError>;
}

pub struct ArtifactService<G, R> {
    sync: ProfileSyncService<G, R>,
}
impl<G, R> ArtifactService<G, R>
where
    G: ProfileGateway + ArtifactGateway,
    R: InventoryRepository + DefinitionRepository,
{
    pub fn new(sync: ProfileSyncService<G, R>) -> Self {
        Self { sync }
    }

    /// Requires definitions, syncs first, then joins a fresh live read by ID.
    /// None selects all committed characters; IDs/classes follow subclass rules.
    pub fn artifact(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ArtifactResult, AppError> {
        ArtifactOperations::artifact(self, selector, policy, now)
    }
}
impl<G, R> ArtifactOperations for ArtifactService<G, R>
where
    G: ProfileGateway + ArtifactGateway,
    R: InventoryRepository + DefinitionRepository,
{
    fn artifact(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ArtifactResult, AppError> {
        if self.sync.repository().current_metadata()?.is_none() {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        self.sync.sync(ProfileSyncReason::Ordinary, policy, now)?;
        let snapshot = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let characters = match selector {
            Some(selector) => vec![
                crate::weekly_selection::select_weekly_character(
                    &snapshot.characters,
                    Some(selector),
                )
                .map_err(|error| {
                    AppError::new(match error {
                        crate::weekly_selection::WeeklySelectionError::InvalidRequest => {
                            ErrorCategory::ProviderValidation
                        }
                        crate::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                            ErrorCategory::AmbiguousMatch
                        }
                    })
                })?,
            ],
            None => snapshot.characters,
        };
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id,
        )?;
        let live = self.sync.gateway_mut().load_artifact(&membership)?;
        live.validate()?;
        let repository = self.sync.repository();
        let artifact = live
            .profile
            .map(|profile| -> Result<_, AppError> {
                Ok(ArtifactSummary {
                    artifact_hash: profile.artifact_hash,
                    name: resolve_name(repository, profile.artifact_hash)?,
                    points_acquired: profile.points_acquired,
                    power_bonus: profile.power_bonus,
                })
            })
            .transpose()?;
        let mut output = Vec::with_capacity(characters.len());
        for character in characters {
            let state = live
                .characters
                .iter()
                .find(|state| state.character_id == character.id)
                .map(|state| -> Result<_, AppError> {
                    let mut tiers = Vec::with_capacity(state.tiers.len());
                    let mut active_perks = Vec::new();
                    for (index, tier) in state.tiers.iter().enumerate() {
                        let mut perks = Vec::with_capacity(tier.perks.len());
                        for perk in &tier.perks {
                            let view = ArtifactPerkView {
                                item_hash: perk.item_hash,
                                name: resolve_name(repository, perk.item_hash)?,
                                active: perk.active,
                            };
                            if view.active {
                                active_perks.push(view.clone());
                            }
                            perks.push(view);
                        }
                        tiers.push(ArtifactTierView {
                            index: index + 1,
                            unlocked: tier.is_unlocked,
                            points_to_unlock: tier.points_to_unlock,
                            perks,
                        });
                    }
                    Ok(CharacterArtifactState {
                        points_used: state.points_used,
                        reset_count: state.reset_count,
                        active_perks,
                        tiers,
                    })
                })
                .transpose()?;
            output.push(CharacterArtifact {
                character_id: character.id,
                class: character.class,
                state,
            });
        }
        Ok(ArtifactResult {
            artifact,
            characters: output,
        })
    }
}
