//! One explicit saved-loadout intent, fresh slot resolution, and one dispatch.
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
    /// False means dispatched, but local invalidation failed; never report success.
    pub local_state_updated: bool,
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
        self.sync.sync(ProfileSyncReason::Preflight, policy, now)?;
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
            snapshot.metadata.membership_id,
        )?;
        // Component 206 is always live, never resolved from cached loadouts.
        let live = self.sync.gateway_mut().load_loadouts(&membership)?;
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
            membership,
            character.id.clone(),
            i32::from(selected.slot) - 1,
        )?;
        let receipt = action.equip_loadout(&provider)?;
        let local_state_updated = self.sync.repository_mut().invalidate_freshness().is_ok();
        Ok(ApplyLoadoutResult {
            character: character.id,
            slot: selected.slot,
            name: selected.name.clone(),
            receipt,
            local_state_updated,
        })
    }
}
