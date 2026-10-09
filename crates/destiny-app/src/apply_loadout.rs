//! One explicit saved-loadout intent, cached slot resolution, and one dispatch.
//! Acknowledged equipment effects retain freshness when complete. Like DIM,
//! we do not update mods/plugs (including subclass aspects/fragments): Bungie
//! does not provide their resulting states.
use crate::{ProfileSyncService, WorldDefinitionRepository};
use destiny_core::{
    AppError, CharacterId, DestinyMembership, EquipLoadoutRequest, ErrorCategory, FreshnessPolicy,
    InventoryRepository, LoadoutActionGateway, LoadoutActionOutcome, LoadoutGateway,
    MoveIntentAuthorization, ProfileGateway, ProfileSyncReason, SearchTerm, Timestamp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadoutSelector {
    Slot(u8),
    Name(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyLoadoutRequest {
    pub character: CharacterId,
    pub loadout: LoadoutSelector,
    pub intent: MoveIntentAuthorization,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutCandidate {
    pub slot: u8,
    pub name: Option<String>,
}
#[derive(Debug, Eq, PartialEq)]
pub enum ApplyLoadoutError {
    App(AppError),
    MissingIntent,
    InvalidSlot,
    EmptySlot,
    UnknownName(Vec<LoadoutCandidate>),
    AmbiguousName(Vec<LoadoutCandidate>),
}
impl From<AppError> for ApplyLoadoutError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyLoadoutResult {
    pub character: CharacterId,
    pub slot: u8,
    pub name: Option<String>,
    pub receipt: LoadoutActionOutcome,
    /// True only when acknowledged equipment effects persisted without invalidation.
    /// Missing/unresolved items, non-applied receipts and storage failures are false.
    pub local_state_updated: bool,
}
/// Loadout acknowledgement may switch an already-held subclass. This does not
/// loosen the weapon/armor slot checks used by single-item equip.
fn loadout_equip_effect<R: destiny_core::DefinitionRepository>(
    repository: &R,
    item: &destiny_core::ItemInstance,
    character: &CharacterId,
    snapshot: &destiny_core::InventorySnapshot,
) -> Result<destiny_core::InventoryEffect, crate::MutationError> {
    let lookup = |hash| match repository.lookup_definition(hash)? {
        destiny_core::Resolution::Unique(definition) => Ok(definition),
        destiny_core::Resolution::Missing => Err(crate::MutationError::NotEquippable),
        destiny_core::Resolution::Ambiguous(_) => Err(crate::MutationError::InvalidProjection),
    };
    if lookup(item.item_hash)?.bucket_hash != Some(destiny_core::SUBCLASS_BUCKET_HASH) {
        return crate::mutation::equip_effect(repository, item, character, snapshot);
    }
    let destination = destiny_core::ItemLocation::Character(character.clone());
    if item.location != destination {
        return Err(crate::MutationError::UnsupportedSource);
    }
    let mut unequip = Vec::new();
    for other in snapshot.items.iter().filter(|other| {
        other.equipped && other.location == destination && other.instance_id != item.instance_id
    }) {
        if lookup(other.item_hash)?.bucket_hash == Some(destiny_core::SUBCLASS_BUCKET_HASH) {
            unequip.push(other.instance_id.clone());
        }
    }
    Ok(destiny_core::InventoryEffect::Equip {
        item: item.instance_id.clone(),
        character: character.clone(),
        unequip,
    })
}

pub struct ApplyLoadoutService<G, R> {
    sync: ProfileSyncService<G, R>,
}
impl<G, R> ApplyLoadoutService<G, R>
where
    G: ProfileGateway + LoadoutGateway,
    R: InventoryRepository + WorldDefinitionRepository,
{
    pub fn new(sync: ProfileSyncService<G, R>) -> Self {
        Self { sync }
    }

    pub fn into_sync(self) -> ProfileSyncService<G, R> {
        self.sync
    }

    pub fn apply<A: LoadoutActionGateway>(
        &mut self,
        action: &mut A,
        request: &ApplyLoadoutRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ApplyLoadoutResult, ApplyLoadoutError> {
        if !request.intent.is_explicit() {
            return Err(ApplyLoadoutError::MissingIntent);
        }
        SearchTerm::new(request.character.as_str())?;
        match &request.loadout {
            LoadoutSelector::Slot(slot) if !(1..=12).contains(slot) => {
                return Err(ApplyLoadoutError::InvalidSlot);
            }
            LoadoutSelector::Name(name) => {
                SearchTerm::new(name)?;
            }
            _ => (),
        }
        if self.sync.repository().current_metadata()?.is_none() {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable).into());
        }
        self.sync.sync(ProfileSyncReason::Ordinary, policy, now)?;
        let snapshot = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let character = crate::weekly_selection::select_weekly_character(
            &snapshot.characters,
            Some(&request.character),
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
        })?;
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id.clone(),
        )?;
        let live = match self.sync.repository().saved_loadouts()? {
            Some(characters) => destiny_core::LoadoutSnapshot { characters },
            None => self.sync.gateway_mut().load_loadouts(&membership)?,
        };
        live.validate()?;
        let mut candidates = Vec::new();
        if let Some(saved) = live
            .characters
            .iter()
            .find(|c| c.character_id == character.id)
        {
            for loadout in &saved.loadouts {
                if !loadout.items.is_empty() {
                    candidates.push(LoadoutCandidate {
                        slot: loadout.slot_index + 1,
                        name: self
                            .sync
                            .repository()
                            .lookup_loadout_name(loadout.name_hash)?,
                    });
                }
            }
        }
        let selected = crate::loadouts::resolve_loadout(&candidates, &request.loadout)?;
        let provider = EquipLoadoutRequest::new(
            membership.clone(),
            character.id.clone(),
            i32::from(selected.slot) - 1,
        )?;
        let receipt = action.equip_loadout(&provider)?;
        let mut complete = receipt.applied();
        let mut effects = Vec::new();
        let mut projected = snapshot;
        if receipt.applied() {
            let saved = live
                .characters
                .iter()
                .find(|saved| saved.character_id == character.id)
                .and_then(|saved| {
                    saved
                        .loadouts
                        .iter()
                        .find(|loadout| loadout.slot_index + 1 == selected.slot)
                })
                .expect("resolved candidate belongs to the saved loadouts");
            for saved_item in &saved.items {
                let mut matches = projected
                    .items
                    .iter()
                    .filter(|item| item.instance_id == saved_item.instance_id);
                let Some(item) = matches.next() else {
                    complete = false;
                    continue;
                };
                if matches.next().is_some() {
                    complete = false;
                    continue;
                }
                // Resolve against accepted effects so same-bucket replacements
                // never reference an item already unequipped by an earlier effect.
                // Subclasses use a loadout-only bucket path, not equip_item policy.
                let effect =
                    loadout_equip_effect(self.sync.repository(), item, &character.id, &projected);
                match effect.and_then(|effect| {
                    destiny_core::apply_inventory_effects(&projected, std::slice::from_ref(&effect))
                        .map(|result| (effect, result.snapshot))
                        .map_err(|_| crate::MutationError::InvalidProjection)
                }) {
                    Ok((effect, updated)) => {
                        effects.push(effect);
                        projected = updated;
                    }
                    Err(_) => complete = false,
                }
            }
        }
        let persisted = crate::mutation::persist_attempt_effects(
            self.sync.repository_mut(),
            &membership,
            &effects,
            complete,
        );
        let local_state_updated = complete && persisted.is_ok();
        Ok(ApplyLoadoutResult {
            character: character.id,
            slot: selected.slot,
            name: selected.name.clone(),
            receipt,
            local_state_updated,
        })
    }
}
