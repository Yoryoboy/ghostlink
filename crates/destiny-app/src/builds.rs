//! Local build use cases. Offline imports never synchronize; inventory sources
//! use the same ordinary freshness gate as saved-loadout reads.
use crate::ProfileSyncService;
use destiny_core::armor_catalog::*;
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
    Analysis(Box<BuildCheckResult>),
    List(Vec<BuildSummary>),
    Build {
        result: Box<BuildSourceResult>,
        dim_json: String,
        source_capture: bool,
    },
    Deleted(bool),
    Export(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildCheckRequest {
    pub selector: BuildSelector,
    pub character: Option<CharacterId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildCheckResult {
    pub summary: BuildSummary,
    pub analysis: build_analysis::BuildAnalysis,
    pub metadata: QueryMetadata,
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
    pub fn check<G, I>(
        &self,
        sync: &mut ProfileSyncService<G, I>,
        selector: &BuildSelector,
        character: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<BuildCheckResult, BuildError>
    where
        G: ProfileGateway,
        I: InventoryRepository
            + WorldDefinitionRepository
            + StatDefinitionRepository
            + ArmorCatalogRepository,
    {
        if sync.repository().current_metadata()?.is_none() {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable).into());
        }
        let outcome = sync.sync(ProfileSyncReason::Ordinary, policy, now)?;
        let snapshot = sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id.clone(),
        )?;
        let build = self.show(&membership, selector)?;
        let candidates: Vec<_> = snapshot
            .characters
            .iter()
            .filter(|c| {
                build.definition.class_type == 3
                    || character_class(c).ok() == Some(build.definition.class_type)
            })
            .cloned()
            .collect();
        let target = if let Some(character) = character {
            crate::weekly_selection::select_weekly_character(&candidates, Some(character)).map_err(
                |e| {
                    AppError::new(match e {
                        crate::weekly_selection::WeeklySelectionError::InvalidRequest => {
                            ErrorCategory::ProviderValidation
                        }
                        crate::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                            ErrorCategory::AmbiguousMatch
                        }
                    })
                },
            )?
        } else {
            match candidates.as_slice() {
                [character] => character.clone(),
                [] => return Err(AppError::new(ErrorCategory::ProviderValidation).into()),
                _ => return Err(AppError::new(ErrorCategory::AmbiguousMatch).into()),
            }
        };
        let mut hashes = std::collections::BTreeSet::new();
        for item in build
            .definition
            .equipped
            .iter()
            .chain(&build.definition.unequipped)
        {
            hashes.insert(item.hash);
            hashes.insert(build_analysis::migrated_hash(item.hash));
            hashes.extend(item.socket_overrides.iter().map(|(_, hash)| *hash));
        }
        if let Some(ExoticArmor::Item(hash)) = build
            .definition
            .parameters
            .as_ref()
            .and_then(|p| p.exotic_armor_hash)
        {
            hashes.insert(hash);
        }
        for item in &snapshot.items {
            hashes.insert(item.item_hash);
            hashes.extend(item.plugs.iter().map(|p| p.plug_hash));
            hashes.extend(item.available_plugs.iter().map(|p| p.plug_hash));
            hashes.extend(item.sockets.iter().filter_map(|s| s.plug_hash));
        }
        let unlocked_mods =
            crate::build_mods::unlocked_mods(&snapshot.plug_set_entries, &target.id);
        if let Some(mods) = build
            .definition
            .parameters
            .as_ref()
            .and_then(|p| p.mods.as_ref())
        {
            hashes.extend(mods.iter().map(|hash| {
                ItemHash::new(i64::from(mod_assignment::resolve_mod_cost_variant(
                    hash.value() as u32,
                    &unlocked_mods,
                )))
            }));
            hashes.extend(mods.iter().map(|hash| {
                ItemHash::new(i64::from(mod_assignment::resolve_mod_cost_variant(
                    hash.value() as u32,
                    &[],
                )))
            }));
            hashes.insert(ItemHash::new(i64::from(
                mod_assignment::DEPRECATED_MOD_PLACEHOLDER,
            )));
        }
        let mut definitions = Vec::new();
        for hash in hashes {
            if let Some(d) = lookup(sync.repository(), hash)? {
                definitions.push(d);
            }
        }
        let mut stats = Vec::new();
        if let Some(query) = build
            .definition
            .parameters
            .as_ref()
            .and_then(|p| p.query.as_deref())
            .and_then(|q| search_query::compile(q).ok())
        {
            for name in search_query::referenced_stats(&query) {
                for hash in sync.repository().stat_hashes_by_name(&name)? {
                    if let Some(d) = sync.repository().lookup_stat(hash)? {
                        stats.push(d);
                    }
                }
            }
        }
        let mod_catalogs = crate::build_mods::catalogs(sync.repository(), &mut definitions)?;
        let objectives = crafting_definitions(sync.repository(), &snapshot.items)?;
        let mut analysis = build_analysis::analyze_build(
            &build.definition,
            &target.id,
            &snapshot.items,
            &definitions,
            &stats,
            &snapshot.characters,
            &objectives,
        );
        // Collect only referenced native/event sets, then evaluate thresholds.
        let counted = build_sets::equipped_set_bonuses(&analysis.items, &definitions, &[]);
        let mut sets = Vec::new();
        for set in counted.sets {
            if let Some(ArmorCatalogRecord::EquipableSet(definition)) = sync
                .repository()
                .lookup_armor_catalog(ArmorCatalogKind::EquipableSet, set.hash)?
            {
                sets.push(definition);
            }
        }
        analysis.equipped_set_bonuses =
            build_sets::equipped_set_bonuses(&analysis.items, &definitions, &sets);
        analysis.mod_fit = mod_assignment::analyze_bucket_mods_with_unlocks(
            &build.definition,
            &analysis.items,
            &snapshot.items,
            &definitions,
            &mod_catalogs,
            &unlocked_mods,
        );
        crate::build_mods::findings(&build.definition, &mut analysis);
        crate::build_stats::evaluate(
            sync.repository(),
            &build.definition,
            &mut analysis,
            &definitions,
            &mod_catalogs,
            &unlocked_mods,
            character_class(&target)?,
        )?;
        Ok(BuildCheckResult {
            summary: build.summary,
            analysis,
            metadata: crate::queries::query_metadata(&outcome, policy, now),
        })
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
        I: InventoryRepository + WorldDefinitionRepository + ArmorCatalogRepository,
    {
        let (snapshot, selected, membership) = source_snapshot(sync, character, policy, now)?;
        let mut definition = BuildDefinition {
            id: self.codec.fresh_id()?,
            name: name.into(),
            class_type: character_class(&selected)?,
            ..Default::default()
        };
        let mut omissions = Vec::new();
        let objectives = crafting_definitions(sync.repository(), &snapshot.items)?;
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
            if item.sockets.is_empty() {
                add_omission(
                    &mut omissions,
                    BuildOmission::InventorySocketIndicesUnavailable,
                );
            }
            if is_armor(bucket) {
                if item.sockets.is_empty() {
                    // Legacy projections cannot establish disabled inserted plugs.
                    collect_mods(
                        sync.repository(),
                        bucket,
                        item.plugs.iter().map(|p| p.plug_hash),
                        &mut mods,
                        &mut fashion,
                    )?;
                } else {
                    for socket in &item.sockets {
                        let Some(hash) = socket.plug_hash else {
                            continue;
                        };
                        let Some(plug) = lookup(sync.repository(), hash)? else {
                            add_omission(&mut omissions, BuildOmission::ItemDefinitionsUnavailable);
                            continue;
                        };
                        // DIM extracts inserted armor mods without an enabled gate.
                        if build_capture::insertable_armor_mod(&plug) {
                            mods.push(hash);
                        }
                        if item_definition.armor.socket_categories.iter().any(|c| {
                            c.socket_category_hash == 1926152773
                                && c.socket_indexes.contains(&u32::from(socket.socket_index))
                        }) {
                            fashion.entry(bucket).or_insert_with(Vec::new).push(hash);
                        }
                    }
                }
            }
            let mut captured = build_item(item, &item_definition, &objectives, &mut omissions);
            if matches!(bucket, SUBCLASS_BUCKET_HASH | ARTIFACT_BUCKET_HASH) {
                let sockets = capture_sockets(sync.repository(), item, &item_definition)?;
                captured.socket_overrides = sockets.overrides;
                if sockets.incomplete {
                    add_omission(&mut omissions, BuildOmission::SocketCategoriesUnavailable);
                    if bucket == ARTIFACT_BUCKET_HASH {
                        add_omission(&mut omissions, BuildOmission::ArtifactUnlocksUnavailable);
                    }
                }
            } else if is_armor(bucket)
                && (item.sockets.is_empty() || item_definition.armor.socket_categories.is_empty())
            {
                add_omission(&mut omissions, BuildOmission::SocketCategoriesUnavailable);
            }
            definition.equipped.push(captured);
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
        let mut omissions = Vec::new();
        let objectives = crafting_definitions(sync.repository(), &snapshot.items)?;
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
            let mut build_item = match &item_definition {
                Some(definition) => build_item(item, definition, &objectives, &mut omissions),
                None => BuildItem {
                    instance_id: Some(item.instance_id.clone()),
                    hash: item.item_hash,
                    amount: Some(item.quantity),
                    socket_overrides: vec![],
                    crafted_date: None,
                },
            };
            if matches!(bucket, Some(SUBCLASS_BUCKET_HASH | ARTIFACT_BUCKET_HASH)) {
                build_item.socket_overrides = build_capture::in_game_socket_overrides(
                    &saved_item
                        .plugs
                        .iter()
                        .map(|p| (u16::from(p.socket_index), p.plug_hash))
                        .collect::<Vec<_>>(),
                );
            } else if bucket.is_some_and(is_armor) {
                let armor = &item_definition.as_ref().expect("armor definition").armor;
                if armor.socket_categories.is_empty() {
                    add_omission(&mut omissions, BuildOmission::SocketCategoriesUnavailable);
                }
                for plug in &saved_item.plugs {
                    if plug.plug_hash.value() == 0 {
                        continue;
                    }
                    let in_category = |hash| {
                        armor.socket_categories.iter().any(|c| {
                            c.socket_category_hash == hash
                                && c.socket_indexes.contains(&u32::from(plug.socket_index))
                        })
                    };
                    if !build_capture::empty_plug(plug.plug_hash.value()) && in_category(590099826)
                    {
                        mods.push(plug.plug_hash);
                    } else if in_category(1926152773) {
                        fashion
                            .entry(bucket.expect("armor bucket"))
                            .or_insert_with(Vec::new)
                            .push(plug.plug_hash);
                    }
                }
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
fn capture_sockets<R: DefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    item: &ItemInstance,
    definition: &ItemDefinition,
) -> Result<build_capture::SocketCapture, AppError> {
    let mut types = Vec::new();
    let mut sets = Vec::new();
    let mut incomplete = false;
    for entry in &definition.armor.socket_entries {
        if !types
            .iter()
            .any(|t: &SocketTypeDefinition| t.hash == entry.socket_type_hash)
            && let Some(ArmorCatalogRecord::SocketType(t)) = repository
                .lookup_armor_catalog(ArmorCatalogKind::SocketType, entry.socket_type_hash)?
        {
            if repository
                .lookup_armor_catalog(ArmorCatalogKind::SocketCategory, t.socket_category_hash)?
                .is_some()
            {
                types.push(t);
            } else {
                incomplete = true;
            }
        }
        if let Some(hash) = entry
            .reusable_plug_set_hash
            .or(entry.randomized_plug_set_hash)
            && !sets.iter().any(|s: &PlugSetDefinition| s.hash == hash)
            && let Some(ArmorCatalogRecord::PlugSet(set)) =
                repository.lookup_armor_catalog(ArmorCatalogKind::PlugSet, hash)?
        {
            // DIM precomputes the empty hash from raw set entries. Membership
            // needs only the inserted plug, whose definition is checked below.
            sets.push(set);
        }
    }
    let capacity = if definition.bucket_hash == Some(SUBCLASS_BUCKET_HASH) {
        crate::armor_model::inspect_fragment_capacity(repository, definition, item)?
            .and_then(|c| c.capacity)
    } else {
        Some(0)
    };
    let mut result = build_capture::equipped_socket_overrides(
        &definition.armor,
        &item.sockets,
        &types,
        &sets,
        capacity,
    );
    // DIM requires a resolved inserted plug definition too.
    let mut overrides = Vec::new();
    for (index, hash) in result.overrides {
        if lookup(repository, hash)?.is_some_and(|d| d.armor.plug_category_hash.is_some()) {
            overrides.push((index, hash));
        } else {
            incomplete = true;
        }
    }
    result.overrides = overrides;
    result.incomplete |= incomplete;
    Ok(result)
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
fn crafting_definitions<R: WorldDefinitionRepository>(
    repository: &R,
    items: &[ItemInstance],
) -> Result<Vec<ObjectiveDefinition>, AppError> {
    let hashes: std::collections::BTreeSet<_> = items
        .iter()
        .flat_map(|i| &i.sockets)
        .filter_map(|s| s.plug_objectives.as_ref())
        .flatten()
        .map(|(hash, _)| *hash)
        .collect();
    let mut definitions = Vec::new();
    for hash in hashes {
        if let Some(definition) = repository.lookup_objective(hash)? {
            definitions.push(definition);
        }
    }
    Ok(definitions)
}

fn build_item(
    item: &ItemInstance,
    definition: &ItemDefinition,
    objectives: &[ObjectiveDefinition],
    omissions: &mut Vec<BuildOmission>,
) -> BuildItem {
    let crafted_date = build_capture::crafted_date(item, definition, objectives);
    if definition
        .armor
        .socket_categories
        .iter()
        .any(|c| c.socket_category_hash == 3583996951)
        && crafted_date.is_none()
    {
        add_omission(omissions, BuildOmission::CraftedDatesUnavailable);
    }
    BuildItem {
        instance_id: Some(item.instance_id.clone()),
        hash: item.item_hash,
        amount: Some(item.quantity),
        socket_overrides: vec![],
        crafted_date,
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
        if build_capture::insertable_armor_mod(&d) {
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
