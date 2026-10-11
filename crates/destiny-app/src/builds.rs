//! Local build use cases. Offline imports never synchronize; inventory sources
//! use the same ordinary freshness gate as saved-loadout reads.
use crate::ProfileSyncService;
use destiny_core::{
    build::*,
    ports::{BuildCodec, BuildRepository, WorldDefinitionRepository},
    *,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildSelector {
    Id(String),
    Name(String),
}
#[derive(Debug, Eq, PartialEq)]
pub enum BuildError {
    App(AppError),
    Codec(DimLoadoutError),
    NotFound,
    AmbiguousName(Vec<BuildSummary>),
    Loadout(crate::ApplyLoadoutError),
    SavedLoadoutsUnavailable,
}
impl From<AppError> for BuildError {
    fn from(e: AppError) -> Self {
        Self::App(e)
    }
}
impl From<DimLoadoutError> for BuildError {
    fn from(e: DimLoadoutError) -> Self {
        Self::Codec(e)
    }
}
impl From<crate::ApplyLoadoutError> for BuildError {
    fn from(e: crate::ApplyLoadoutError) -> Self {
        Self::Loadout(e)
    }
}
impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::App(e) => e.fmt(f),
            Self::Codec(e) => e.fmt(f),
            Self::NotFound => f.write_str("build not found"),
            Self::AmbiguousName(_) => f.write_str("ambiguous build name"),
            Self::Loadout(_) => f.write_str("could not resolve saved loadout"),
            Self::SavedLoadoutsUnavailable => {
                f.write_str("saved loadouts unavailable; sync profile first")
            }
        }
    }
}
impl std::error::Error for BuildError {}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildOmission {
    InventorySocketIndicesUnavailable,
    ArtifactUnlocksUnavailable,
    SocketCategoriesUnavailable,
    CraftedDatesUnavailable,
    ItemDefinitionsUnavailable,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildSourceResult {
    pub build: StoredBuild,
    /// DIM drops unresolved saved entries; report them rather than substitute.
    pub unresolved_items: Vec<ItemInstanceId>,
    pub omissions: Vec<BuildOmission>,
}
/// Transport-independent local build command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalBuildRequest {
    Import(String),
    List,
    Show(BuildSelector),
    Delete(BuildSelector),
    Export(BuildSelector),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildReply {
    List(Vec<BuildSummary>),
    Build {
        result: Box<BuildSourceResult>,
        dim_json: String,
        source_capture: bool,
    },
    Deleted(bool),
    Export(String),
}

pub struct BuildService<R, C> {
    repository: R,
    codec: C,
}
impl<R: BuildRepository, C: BuildCodec> BuildService<R, C> {
    pub fn new(repository: R, codec: C) -> Self {
        Self { repository, codec }
    }
    pub fn execute_local(
        &mut self,
        membership: &DestinyMembership,
        request: &LocalBuildRequest,
        now: Timestamp,
    ) -> Result<BuildReply, BuildError> {
        match request {
            LocalBuildRequest::List => Ok(BuildReply::List(self.list(membership)?)),
            LocalBuildRequest::Delete(selector) => {
                Ok(BuildReply::Deleted(self.delete(membership, selector)?))
            }
            LocalBuildRequest::Export(selector) => {
                Ok(BuildReply::Export(self.export(membership, selector)?))
            }
            LocalBuildRequest::Import(input) => {
                let build = self.import(membership, input, now)?;
                self.reply(BuildSourceResult {
                    build,
                    unresolved_items: vec![],
                    omissions: vec![],
                })
            }
            LocalBuildRequest::Show(selector) => {
                let build = self.show(membership, selector)?;
                let dim_json = self.codec.export(&build.definition)?;
                Ok(BuildReply::Build {
                    result: Box::new(BuildSourceResult {
                        build,
                        unresolved_items: vec![],
                        omissions: vec![],
                    }),
                    dim_json,
                    source_capture: false,
                })
            }
        }
    }
    pub fn reply(&self, result: BuildSourceResult) -> Result<BuildReply, BuildError> {
        let dim_json = self.codec.export(&result.build.definition)?;
        Ok(BuildReply::Build {
            result: Box::new(result),
            dim_json,
            source_capture: true,
        })
    }
    pub fn into_parts(self) -> (R, C) {
        (self.repository, self.codec)
    }
    pub fn import(
        &mut self,
        membership: &DestinyMembership,
        input: &str,
        now: Timestamp,
    ) -> Result<StoredBuild, BuildError> {
        let build = self.codec.import(input)?;
        Ok(self
            .repository
            .save_build(membership, &build, BuildSource::Import, now)?)
    }
    pub fn list(&self, membership: &DestinyMembership) -> Result<Vec<BuildSummary>, BuildError> {
        Ok(self.repository.list_builds(membership)?)
    }
    pub fn show(
        &self,
        membership: &DestinyMembership,
        selector: &BuildSelector,
    ) -> Result<StoredBuild, BuildError> {
        let id = match selector {
            BuildSelector::Id(id) => id.clone(),
            BuildSelector::Name(name) => {
                match self.repository.find_build_by_name(membership, name)? {
                    Resolution::Missing => return Err(BuildError::NotFound),
                    Resolution::Unique(b) => b.id,
                    Resolution::Ambiguous(v) => return Err(BuildError::AmbiguousName(v)),
                }
            }
        };
        self.repository
            .get_build(membership, &id)?
            .ok_or(BuildError::NotFound)
    }
    pub fn delete(
        &mut self,
        membership: &DestinyMembership,
        selector: &BuildSelector,
    ) -> Result<bool, BuildError> {
        // Direct ID deletion is idempotent; name deletion still requires uniqueness.
        let id = match selector {
            BuildSelector::Id(id) => id.clone(),
            BuildSelector::Name(_) => self.show(membership, selector)?.definition.id,
        };
        Ok(self.repository.delete_build(membership, &id)?)
    }
    pub fn export(
        &self,
        membership: &DestinyMembership,
        selector: &BuildSelector,
    ) -> Result<String, BuildError> {
        Ok(self
            .codec
            .export(&self.show(membership, selector)?.definition)?)
    }
    pub fn from_equipped<G, I>(
        &mut self,
        sync: &mut ProfileSyncService<G, I>,
        character: &CharacterId,
        name: &str,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<BuildSourceResult, BuildError>
    where
        G: ProfileGateway,
        I: InventoryRepository + WorldDefinitionRepository,
    {
        let (snapshot, selected, membership) = source_snapshot(sync, character, policy, now)?;
        let mut definition = BuildDefinition {
            id: self.codec.fresh_id()?,
            name: name.into(),
            class_type: character_class(&selected)?,
            ..Default::default()
        };
        let mut omissions = vec![
            BuildOmission::InventorySocketIndicesUnavailable,
            BuildOmission::ArtifactUnlocksUnavailable,
            BuildOmission::SocketCategoriesUnavailable,
            BuildOmission::CraftedDatesUnavailable,
        ];
        let mut mods = Vec::new();
        let mut fashion = BTreeMap::new();
        for item in snapshot
            .items
            .iter()
            .filter(|i| i.equipped && i.location == ItemLocation::Character(selected.id.clone()))
        {
            let Some(item_definition) = lookup(sync.repository(), item.item_hash)? else {
                add_omission(&mut omissions, BuildOmission::ItemDefinitionsUnavailable);
                continue;
            };
            if !snapshot_bucket(item_definition.bucket_hash) {
                continue;
            }
            let bucket = item_definition.bucket_hash.expect("recognized bucket");
            if is_armor(bucket) {
                collect_mods(
                    sync.repository(),
                    bucket,
                    item.plugs.iter().map(|p| p.plug_hash),
                    &mut mods,
                    &mut fashion,
                )?;
            }
            // PlugSummary has no socket index. Never use its ordinal as an index.
            definition.equipped.push(build_item(item));
        }
        if !mods.is_empty() || !fashion.is_empty() {
            definition.parameters = Some(BuildParameters {
                mods: (!mods.is_empty()).then_some(mods),
                mods_by_bucket: (!fashion.is_empty()).then_some(fashion),
                ..Default::default()
            });
        }
        let build =
            self.repository
                .save_build(&membership, &definition, BuildSource::Equipped, now)?;
        Ok(BuildSourceResult {
            build,
            unresolved_items: vec![],
            omissions,
        })
    }
    pub fn from_loadout<G, I>(
        &mut self,
        sync: &mut ProfileSyncService<G, I>,
        character: &CharacterId,
        selector: &crate::LoadoutSelector,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<BuildSourceResult, BuildError>
    where
        G: ProfileGateway,
        I: InventoryRepository + WorldDefinitionRepository,
    {
        let (snapshot, selected, membership) = source_snapshot(sync, character, policy, now)?;
        let saved = sync
            .repository()
            .saved_loadouts()?
            .ok_or(BuildError::SavedLoadoutsUnavailable)?;
        LoadoutSnapshot {
            characters: saved.clone(),
        }
        .validate()?;
        let loadouts = saved
            .iter()
            .find(|c| c.character_id == selected.id)
            .map(|c| c.loadouts.as_slice())
            .unwrap_or_default();
        let candidates = loadouts
            .iter()
            .map(|l| {
                Ok(crate::LoadoutCandidate {
                    slot: l.slot_index + 1,
                    name: sync.repository().lookup_loadout_name(l.name_hash)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let candidate = crate::loadouts::resolve_loadout(&candidates, selector)?;
        let loadout = loadouts
            .iter()
            .find(|l| l.slot_index + 1 == candidate.slot)
            .ok_or(BuildError::NotFound)?;
        let mut definition = BuildDefinition {
            id: self.codec.fresh_id()?,
            name: candidate.name.unwrap_or_else(|| "Unknown".into()),
            class_type: character_class(&selected)?,
            ..Default::default()
        };
        let mut unresolved_items = Vec::new();
        let mut mods = Vec::new();
        let mut fashion = BTreeMap::new();
        let mut omissions = vec![
            BuildOmission::SocketCategoriesUnavailable,
            BuildOmission::CraftedDatesUnavailable,
        ];
        for saved_item in &loadout.items {
            let Some(item) = snapshot
                .items
                .iter()
                .find(|i| i.instance_id == saved_item.instance_id)
            else {
                unresolved_items.push(saved_item.instance_id.clone());
                continue;
            };
            let item_definition = lookup(sync.repository(), item.item_hash)?;
            let bucket = item_definition.as_ref().and_then(|d| d.bucket_hash);
            if bucket.is_none() {
                add_omission(&mut omissions, BuildOmission::ItemDefinitionsUnavailable);
            }
            let mut build_item = build_item(item);
            if matches!(bucket, Some(SUBCLASS_BUCKET_HASH | ARTIFACT_BUCKET_HASH)) {
                build_item.socket_overrides = saved_item
                    .plugs
                    .iter()
                    .map(|p| (u16::from(p.socket_index), p.plug_hash))
                    .collect();
                build_item.socket_overrides.sort_by_key(|p| p.0);
            } else if bucket.is_some_and(is_armor) {
                collect_mods(
                    sync.repository(),
                    bucket.expect("armor bucket"),
                    saved_item.plugs.iter().map(|p| p.plug_hash),
                    &mut mods,
                    &mut fashion,
                )?;
            }
            definition.equipped.push(build_item);
        }
        definition.parameters = Some(BuildParameters {
            mods: Some(mods),
            mods_by_bucket: Some(fashion),
            in_game_identifiers: Some(BuildInGameIdentifiers {
                name_hash: loadout.name_hash,
                color_hash: loadout.color_hash,
                icon_hash: loadout.icon_hash,
            }),
            ..Default::default()
        });
        let build = self.repository.save_build(
            &membership,
            &definition,
            BuildSource::InGameLoadout,
            now,
        )?;
        Ok(BuildSourceResult {
            build,
            unresolved_items,
            omissions,
        })
    }
}
fn source_snapshot<G: ProfileGateway, R: InventoryRepository + WorldDefinitionRepository>(
    sync: &mut ProfileSyncService<G, R>,
    character: &CharacterId,
    policy: FreshnessPolicy,
    now: Timestamp,
) -> Result<(InventorySnapshot, Character, DestinyMembership), BuildError> {
    if sync.repository().current_metadata()?.is_none() {
        return Err(AppError::new(ErrorCategory::DefinitionsUnavailable).into());
    }
    sync.sync(ProfileSyncReason::Ordinary, policy, now)?;
    let snapshot = sync
        .repository()
        .load_current()?
        .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
    let selected =
        crate::weekly_selection::select_weekly_character(&snapshot.characters, Some(character))
            .map_err(|e| {
                AppError::new(match e {
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
    Ok((snapshot, selected, membership))
}
fn character_class(character: &Character) -> Result<u8, AppError> {
    match character.class.as_deref() {
        Some("Titan") => Ok(0),
        Some("Hunter") => Ok(1),
        Some("Warlock") => Ok(2),
        _ => Err(AppError::new(ErrorCategory::ProviderValidation)),
    }
}
fn build_item(item: &ItemInstance) -> BuildItem {
    BuildItem {
        instance_id: Some(item.instance_id.clone()),
        hash: item.item_hash,
        amount: Some(item.quantity),
        socket_overrides: vec![],
        crafted_date: None,
    }
}
fn lookup<R: DefinitionRepository>(
    repository: &R,
    hash: ItemHash,
) -> Result<Option<ItemDefinition>, AppError> {
    match repository.lookup_definition(hash)? {
        Resolution::Unique(d) => Ok(Some(d)),
        Resolution::Missing => Ok(None),
        Resolution::Ambiguous(_) => Err(AppError::new(ErrorCategory::LocalStorage)),
    }
}
fn is_armor(bucket: BucketHash) -> bool {
    domain::ItemSlot::from_equipment_bucket(bucket).is_some_and(|s| {
        matches!(
            s,
            domain::ItemSlot::Helmet
                | domain::ItemSlot::Gauntlets
                | domain::ItemSlot::Chest
                | domain::ItemSlot::Legs
                | domain::ItemSlot::ClassItem
        )
    })
}
fn snapshot_bucket(bucket: Option<BucketHash>) -> bool {
    bucket.is_some_and(|b| {
        domain::ItemSlot::from_equipment_bucket(b).is_some()
            || b == SUBCLASS_BUCKET_HASH
            || b == ARTIFACT_BUCKET_HASH
            || matches!(b.value(), 4023194814 | 284967655 | 2025709351 | 4274335291)
    })
}
fn collect_mods<R: DefinitionRepository>(
    repository: &R,
    bucket: BucketHash,
    plugs: impl Iterator<Item = ItemHash>,
    mods: &mut Vec<ItemHash>,
    fashion: &mut BTreeMap<BucketHash, Vec<ItemHash>>,
) -> Result<(), AppError> {
    for hash in plugs {
        let Some(d) = lookup(repository, hash)? else {
            continue;
        };
        if query::is_named_armor_mod(&d) {
            mods.push(hash);
        } else if d.plug_category.as_deref().is_some_and(|c| {
            c.starts_with("shader") || c.contains("skins") || c.contains("ornament")
        }) {
            fashion.entry(bucket).or_default().push(hash);
        }
    }
    Ok(())
}
fn add_omission(omissions: &mut Vec<BuildOmission>, value: BuildOmission) {
    if !omissions.contains(&value) {
        omissions.push(value);
    }
}
