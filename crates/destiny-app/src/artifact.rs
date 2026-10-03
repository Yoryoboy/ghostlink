//! Live seasonal artifact query over freshness-synchronized characters.
use crate::ProfileSyncService;
use destiny_core::{
    AppError, ArtifactGateway, CharacterId, DefinitionRepository, DestinyMembership, ErrorCategory,
    FreshnessPolicy, InventoryRepository, ItemHash, ItemInstanceId, ProfileGateway,
    ProfileSyncReason, Resolution, Timestamp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactResult {
    pub points: Option<ArtifactPoints>,
    pub characters: Vec<CharacterArtifact>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPoints {
    pub points: u32,
    pub points_cap: u32,
    pub progress_to_next_level: u32,
    pub next_level_at: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterArtifact {
    pub character_id: CharacterId,
    pub class: Option<String>,
    /// Equipped artifact and sockets from the synced inventory, independent of live state.
    pub equipped: Option<EquippedArtifact>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquippedArtifact {
    pub instance_id: ItemInstanceId,
    pub item_hash: ItemHash,
    pub name: Option<String>,
    pub slots: Vec<ArtifactSlot>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSlot {
    pub plug_hash: ItemHash,
    pub name: Option<String>,
    pub empty: bool,
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
        let repository = self.sync.repository();
        let points = live.profile.map(|profile| ArtifactPoints {
            points: profile.points,
            points_cap: profile.points_cap,
            progress_to_next_level: profile.progress_to_next_level,
            next_level_at: profile.next_level_at,
        });
        let mut output = Vec::with_capacity(characters.len());
        for character in characters {
            let mut equipped = None;
            for instance in snapshot.items.iter().filter(|instance| {
                instance.equipped
                    && instance.location
                        == destiny_core::ItemLocation::Character(character.id.clone())
            }) {
                let definition = match repository.lookup_definition(instance.item_hash)? {
                    Resolution::Unique(definition) => definition,
                    Resolution::Missing => continue,
                    Resolution::Ambiguous(_) => {
                        return Err(AppError::new(ErrorCategory::LocalStorage));
                    }
                };
                if definition.bucket_hash != Some(destiny_core::ARTIFACT_BUCKET_HASH) {
                    continue;
                }
                if equipped.is_some() {
                    return Err(AppError::new(ErrorCategory::LocalStorage));
                }
                let mut slots = Vec::new();
                for plug in &instance.plugs {
                    let definition = match repository.lookup_definition(plug.plug_hash)? {
                        Resolution::Unique(definition) => definition,
                        Resolution::Missing => continue,
                        Resolution::Ambiguous(_) => {
                            return Err(AppError::new(ErrorCategory::LocalStorage));
                        }
                    };
                    if definition.plug_category.as_deref() != Some("artifact_perks") {
                        continue;
                    }
                    slots.push(ArtifactSlot {
                        plug_hash: plug.plug_hash,
                        empty: definition.name.starts_with("Empty "),
                        name: Some(definition.name),
                    });
                }
                equipped = Some(EquippedArtifact {
                    instance_id: instance.instance_id.clone(),
                    item_hash: instance.item_hash,
                    name: Some(definition.name),
                    slots,
                });
            }
            output.push(CharacterArtifact {
                character_id: character.id,
                class: character.class,
                equipped,
            });
        }
        Ok(ArtifactResult {
            points,
            characters: output,
        })
    }
}
