//! Shared application orchestration for `list_characters`, `search_inventory`,
//! and `inspect_item`.
//!
//! The service is provider- and transport-neutral. It composes a single
//! [`ProfileSyncService`] over one repository so every read runs through the
//! same freshness policy before it answers: an ordinary request over a fresh
//! projection skips acquisition, while a missing, stale, or mis-sourced
//! projection is refreshed first. The service never duplicates the skip,
//! membership-verification, publication, or revision logic owned by the sync
//! service and the repository. It reads the committed metadata the repository
//! reports and never stamps `checked_at`, `source`, `definition_version`, or a
//! revision itself, so no parallel freshness or revision concept is introduced.
//!
//! # Read composition
//!
//! Every query shares one preparation step:
//!
//! 1. **Validate before any side effect.** A malformed search request is rejected
//!    with the sanitized [`ErrorCategory::ProviderValidation`] category before
//!    the service attempts a refresh, so an invalid query can never trigger a
//!    provider call or a projection mutation.
//! 2. **Synchronize first.** The service calls
//!    `ProfileSyncService::sync(Ordinary, policy, now)` with the caller-supplied
//!    policy and explicit `now`. A fresh projection skips acquisition without a
//!    gateway call; a missing, stale, or mis-sourced projection refreshes before
//!    any query runs. If the refresh fails, its sanitized error propagates
//!    unchanged and no query is issued against the prior committed projection.
//! 3. **Read the committed projection.** Definition and instance reads go
//!    through the core-owned [`SearchRepository`] and [`DefinitionRepository`]
//!    seams; the character listing reads the committed [`InventoryRepository`]
//!    snapshot. Nothing here re-implements matching, ordering, or truncation.
//!
//! # Identity and metadata
//!
//! [`ItemHash`] and [`ItemInstanceId`] stay structurally distinct: definition
//! search returns [`ItemDefinition`] candidates, instance search returns
//! concrete instances, and every hit pairs the instance with the exact
//! definition its `item_hash` references. Search never selects a single instance from an
//! ambiguous display name; it returns the bounded candidate and hit lists from
//! [`InventorySearchResult`]. Inspection additionally requires a concrete
//! instance identifier and fails closed on an invalid repository state rather
//! than choosing among duplicates.
//!
//! Every answer carries a [`QueryMetadata`] derived from the committed
//! post-sync profile snapshot the repository reports: the profile revision,
//! `checked_at`, and `source` under [`Dataset::ProfileInventory`], together with
//! the current [`FreshnessStatus`] evaluated at the caller-supplied `now`, advanced
//! to at least the committed `checked_at` when synchronization refreshed. The
//! profile revision is never conflated with the Manifest definition revision.
//! No raw provider payload, credential, membership identifier, or unbounded
//! upstream record is retained, returned, or rendered.

use destiny_core::{
    AppError, CharacterListResult, Dataset, DefinitionRepository, DefinitionSearchQuery,
    ErrorCategory, FreshnessInput, FreshnessPolicy, InspectItemRequest, InstanceSearchQuery,
    InventoryRepository, InventorySearchHit, InventorySearchResult, InventorySnapshot,
    ItemDefinition, ItemHash, ItemInspection, ProfileGateway, ProfileSyncOutcome,
    ProfileSyncReason, QueryMetadata, Resolution, SearchRepository, SyncMetadata, Timestamp,
};

use crate::sync::ProfileSyncService;

/// Request-local definition caches shared by search and exact inspection.
/// Missing plug/stat definitions are cached and omitted, just as in search.
#[derive(Default)]
struct InstanceEnrichment {
    plugs: std::collections::HashMap<ItemHash, Option<ItemDefinition>>,
    stats: std::collections::HashMap<u32, Option<destiny_core::StatDefinition>>,
}

impl InstanceEnrichment {
    fn resolve_plugs(
        &mut self,
        repository: &impl DefinitionRepository,
        hashes: impl Iterator<Item = ItemHash>,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        let mut output = Vec::new();
        for hash in hashes {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.plugs.entry(hash) {
                let resolved = match repository.lookup_definition(hash)? {
                    Resolution::Unique(definition) => Some(definition),
                    Resolution::Missing => None,
                    Resolution::Ambiguous(_) => return Err(invalid_repository_state()),
                };
                entry.insert(resolved);
            }
            if let Some(definition) = &self.plugs[&hash] {
                output.push(definition.clone());
            }
        }
        Ok(output)
    }

    fn resolve(
        &mut self,
        repository: &(impl DefinitionRepository + destiny_core::StatDefinitionRepository),
        definition: ItemDefinition,
        instance: destiny_core::ItemInstance,
    ) -> Result<InventorySearchHit, AppError> {
        let plugs =
            self.resolve_plugs(repository, instance.plugs.iter().map(|plug| plug.plug_hash))?;
        let available = self.resolve_plugs(
            repository,
            instance.available_plugs.iter().map(|plug| plug.plug_hash),
        )?;
        let (available_perks, available_mods) =
            destiny_core::query::inventory_available_names(&definition, &plugs, &available);
        let (perk_names, mod_names) =
            destiny_core::query::inventory_plug_names(&definition, &plugs);
        let mut named = Vec::new();
        for stat in &instance.stats {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.stats.entry(stat.stat_hash)
            {
                entry.insert(repository.lookup_stat(stat.stat_hash)?);
            }
            if let Some(definition) = &self.stats[&stat.stat_hash] {
                named.push((definition.name().to_owned(), stat.stat_hash, stat.value));
            }
        }
        named.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
        let stats = named
            .into_iter()
            .map(|(name, _, value)| destiny_core::NamedStat { name, value })
            .collect();
        Ok(InventorySearchHit {
            available_perks,
            available_mods,
            archetype: destiny_core::query::inventory_archetype(&definition, &plugs),
            stats,
            definition,
            perk_names,
            mod_names,
            instance,
        })
    }
}

/// A provider-neutral read request for `search_inventory`.
///
/// It carries the two independent query shapes the core contract already
/// defines, so the application layer adds no new matching or filter concept:
///
/// - `definitions` is always run. Its candidates are the bounded definition-level
///   matches in [`ItemHash`] order.
/// - `instances` is optional. Definition filters create an instance query when
///   absent, using the definition query's limit. When present, hash, perk,
///   character, power and limit constraints are preserved. Active request-level
///   definition criteria supply the instance query's definition criteria, without
///   intersecting a truncated candidate hash list. Hits pair each instance with
///   its exact definition in [`ItemInstanceId`] order.
/// - With neither definition filters nor an instance query, `hits` is empty.
///
/// The service validates both queries before it refreshes anything.
///
/// [`ItemInstanceId`]: destiny_core::ItemInstanceId
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchInventoryRequest {
    pub definitions: DefinitionSearchQuery,
    pub instances: Option<InstanceSearchQuery>,
}

/// Orchestrates one freshness-aware read over a synchronized projection.
///
/// The service borrows a [`ProfileSyncService`] and reaches the visible
/// projection only through its [`ProfileSyncService::repository`] accessor, so
/// exactly one repository instance answers every definition, instance, and
/// character read. `R` therefore has to provide all three read seams at once.
pub struct QueryService<G, R> {
    sync: ProfileSyncService<G, R>,
}

impl<G, R> QueryService<G, R>
where
    G: ProfileGateway,
    R: InventoryRepository
        + SearchRepository
        + DefinitionRepository
        + destiny_core::StatDefinitionRepository,
{
    /// Wraps one profile sync service and the repository behind it.
    pub fn new(sync: ProfileSyncService<G, R>) -> Self {
        Self { sync }
    }

    /// Returns the composed sync service for a subsequent authenticated read.
    pub fn into_sync(self) -> ProfileSyncService<G, R> {
        self.sync
    }

    /// Borrows the composed sync service.
    pub fn sync(&self) -> &ProfileSyncService<G, R> {
        &self.sync
    }

    /// Mutably borrows the composed sync service for test or composition setup.
    pub fn sync_mut(&mut self) -> &mut ProfileSyncService<G, R> {
        &mut self.sync
    }

    /// Borrows the single repository answering every read.
    pub fn repository(&self) -> &R {
        self.sync.repository()
    }

    /// Mutably borrows the repository for test or composition setup.
    pub fn repository_mut(&mut self) -> &mut R {
        self.sync.repository_mut()
    }

    /// Lists the committed characters after synchronizing the projection.
    ///
    /// The result preserves the repository-owned canonical character order and
    /// carries the committed post-sync metadata and current freshness. An
    /// ordinary request over a fresh projection performs no gateway call.
    pub fn list_characters(
        &mut self,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<CharacterListResult, AppError> {
        let outcome = self.synchronize(policy, now)?;
        let metadata = query_metadata(&outcome, policy, now);
        let snapshot = self.committed_snapshot()?;
        Ok(CharacterListResult {
            characters: snapshot.characters,
            metadata,
        })
    }

    /// Returns equipped subclasses after requiring definitions and synchronizing.
    ///
    /// No selector returns all characters in committed order. An explicit
    /// selector uses the shared character selection rules (exact ID first,
    /// otherwise a case-insensitive, unambiguous class name). Unknown plug
    /// definitions are omitted: without a category their kind is unknowable.
    /// Multiple equipped subclass items fail closed as invalid repository state.
    pub fn character_subclasses(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassResult, AppError> {
        self.require_definitions()?;
        self.synchronize(policy, now)?;
        let snapshot = self.committed_snapshot()?;
        let characters = match character_id {
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
        let mut output = Vec::with_capacity(characters.len());
        let mut enrichment = InstanceEnrichment::default();
        for character in characters {
            let mut subclass = None;
            for instance in snapshot.items.iter().filter(|instance| {
                instance.equipped
                    && instance.location
                        == destiny_core::ItemLocation::Character(character.id.clone())
            }) {
                let definition = self.resolve_definition(instance.item_hash)?;
                if definition.bucket_hash != Some(destiny_core::SUBCLASS_BUCKET_HASH) {
                    continue;
                }
                if subclass.is_some() {
                    return Err(invalid_repository_state());
                }
                let plugs = enrichment.resolve_plugs(
                    self.repository(),
                    instance.plugs.iter().map(|plug| plug.plug_hash),
                )?;
                let slots = plugs
                    .into_iter()
                    .filter_map(|plug| {
                        let kind =
                            destiny_core::classify_subclass_plug(plug.plug_category.as_deref()?)?;
                        Some(destiny_core::SubclassSlot {
                            kind,
                            plug_hash: plug.hash,
                            empty: plug.name.starts_with("Empty ")
                                && plug.name.ends_with(" Socket"),
                            name: Some(plug.name),
                        })
                    })
                    .collect();
                subclass = Some(destiny_core::SubclassConfig {
                    instance_id: instance.instance_id.clone(),
                    item_hash: instance.item_hash,
                    name: definition.name,
                    element: definition.element,
                    slots,
                });
            }
            output.push(destiny_core::CharacterSubclass {
                character_id: character.id,
                class: character.class,
                subclass,
            });
        }
        Ok(destiny_core::SubclassResult { characters: output })
    }

    /// Searches definitions, and optionally concrete instances, after syncing.
    ///
    /// Both queries are validated before the refresh so a malformed request
    /// never triggers a provider call. Definition matches are returned as
    /// [`InventorySearchResult::candidates`]; each instance match is paired with
    /// the exact definition its `item_hash` references. An instance that
    /// references a missing or duplicated definition is an invalid repository
    /// state and fails closed rather than returning a partial hit.
    pub fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<InventorySearchResult, AppError> {
        request.definitions.validate()?;
        if let Some(instances) = &request.instances {
            instances.validate()?;
        }

        self.require_definitions()?;
        let mut request = request.clone();
        let mut resolved = destiny_core::name_matching::ResolvedNames::default();
        if let Some(query) = &mut request.instances {
            for (kind, terms, output, per_term) in [
                (
                    "perk",
                    &mut query.perk_name,
                    &mut resolved.perk,
                    &mut resolved.perk_terms,
                ),
                (
                    "mod",
                    &mut query.mod_name,
                    &mut resolved.mods,
                    &mut resolved.mod_terms,
                ),
            ] {
                if let Some(terms) = terms {
                    let catalog = self.repository().name_catalog(kind)?;
                    for term in terms {
                        let names = term.resolve(&catalog, kind)?;
                        output.extend(names.iter().cloned());
                        per_term.push(names);
                    }
                }
            }
            if let Some(term) = &mut query.archetype {
                resolved.archetype =
                    term.resolve(&self.repository().name_catalog("archetype")?, "archetype")?;
            }
            if let Some(sort) = &mut query.sort {
                if self.repository().stat_definition_count()? == 0 {
                    return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
                }
                let name = sort.resolve(&self.repository().name_catalog("stat")?)?;
                resolved.sort = Some((name, sort.order().to_owned()));
            }
            if !query.stats.is_empty() {
                if self.repository().stat_definition_count()? == 0 {
                    return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
                }
                let catalog = self.repository().name_catalog("stat")?;
                for condition in &mut query.stats {
                    let input = condition.name.as_str().to_owned();
                    let names = condition.name.resolve(&catalog, "stat")?;
                    resolved.stats.insert(input, names);
                }
            }
        }
        let outcome = self.synchronize(policy, now)?;
        let metadata = query_metadata(&outcome, policy, now);

        let candidates = self
            .sync
            .repository()
            .search_definitions(&request.definitions)?;

        let instance_query = if request.definitions.has_filters() {
            let mut query = request.instances.clone().unwrap_or(InstanceSearchQuery {
                sort: None,
                location: None,
                equipped: None,
                include_available: false,
                item_hashes: destiny_core::ItemHashFilter::Any,
                perk_hash: None,
                perk_name: None,
                mod_name: None,
                archetype: None,
                stats: Vec::new(),
                character_id: None,
                definitions: None,
                min_power: None,
                limit: request.definitions.limit,
            });
            query.definitions = Some(Box::new(request.definitions.clone()));
            Some(query)
        } else {
            request.instances.clone()
        };
        let mut hits = Vec::new();
        if let Some(instance_query) = &instance_query {
            let instances = self.sync.repository().search_instances(instance_query)?;
            hits.reserve(instances.len());
            let mut enrichment = InstanceEnrichment::default();
            for instance in instances {
                let definition = self.resolve_definition(instance.item_hash)?;
                hits.push(enrichment.resolve(self.repository(), definition, instance)?);
            }
        }

        Ok(InventorySearchResult {
            resolved,
            candidates,
            hits,
            metadata,
        })
    }

    /// Inspects exactly one concrete instance after syncing the projection.
    ///
    /// The request is keyed by a concrete [`ItemInstanceId`]. A missing instance
    /// reports [`Resolution::Missing`]; an unambiguous instance reports
    /// [`Resolution::Unique`] with its definition and current state. A duplicated
    /// instance identifier, or a definition that is missing or duplicated for
    /// the instance's `item_hash`, is an invalid repository state and fails
    /// closed with the sanitized [`ErrorCategory::LocalStorage`] category.
    pub fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<Resolution<ItemInspection>, AppError> {
        self.require_definitions()?;
        let outcome = self.synchronize(policy, now)?;
        let metadata = query_metadata(&outcome, policy, now);

        let instance = match self
            .sync
            .repository()
            .lookup_instance(&request.instance_id)?
        {
            Resolution::Missing => return Ok(Resolution::Missing),
            Resolution::Unique(instance) => instance,
            Resolution::Ambiguous(_) => return Err(invalid_repository_state()),
        };
        let definition = self.resolve_definition(instance.item_hash)?;
        let hit = InstanceEnrichment::default().resolve(self.repository(), definition, instance)?;
        Ok(Resolution::Unique(ItemInspection {
            perk_names: hit.perk_names,
            mod_names: hit.mod_names,
            archetype: hit.archetype,
            available_perks: hit.available_perks,
            available_mods: hit.available_mods,
            stats: hit.stats,
            definition: hit.definition,
            instance: hit.instance,
            metadata,
        }))
    }

    fn require_definitions(&self) -> Result<(), AppError> {
        if self.repository().current_metadata()?.is_none() {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        Ok(())
    }

    /// Runs one ordinary policy-driven synchronization.
    ///
    /// This is the single composition point with [`ProfileSyncService`]: the
    /// service never re-implements the freshness gate or the refresh path, and
    /// it never forces a refresh reason of its own.
    fn synchronize(
        &mut self,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ProfileSyncOutcome, AppError> {
        self.sync.sync(ProfileSyncReason::Ordinary, policy, now)
    }

    /// Reads the committed snapshot or reports an invalid repository state.
    ///
    /// A successful synchronization always leaves a committed projection, so a
    /// missing snapshot here can only mean the repository lost state between the
    /// decision and the read.
    fn committed_snapshot(&self) -> Result<InventorySnapshot, AppError> {
        self.sync
            .repository()
            .load_current()?
            .ok_or_else(invalid_repository_state)
    }

    /// Resolves one exact definition or fails closed on an invalid projection.
    fn resolve_definition(&self, hash: ItemHash) -> Result<ItemDefinition, AppError> {
        match self.sync.repository().lookup_definition(hash)? {
            Resolution::Unique(definition) => Ok(definition),
            Resolution::Missing | Resolution::Ambiguous(_) => Err(invalid_repository_state()),
        }
    }
}

/// The narrow, object-safe read surface a presentation boundary dispatches to.
///
/// It exposes only the committed reads and keeps the caller-supplied
/// [`FreshnessPolicy`] and explicit [`Timestamp`] in every signature. A caller
/// therefore forwards its own policy and time and never reproduces the
/// freshness, authorization, or synchronization decision owned by the shared
/// service: an implementation of this trait delegates to [`QueryService`] and
/// decides nothing beyond that delegation. The trait has no generic methods,
/// associated types, or `Self`-typed returns, so `&mut dyn QueryOperations` is
/// a valid dispatch target.
pub trait QueryOperations {
    /// Reads equipped subclasses for one selector, or all characters.
    fn character_subclasses(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassResult, AppError>;

    /// Lists the committed characters after synchronizing the projection.
    fn list_characters(
        &mut self,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<CharacterListResult, AppError>;

    /// Searches definitions and, optionally, concrete instances.
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<InventorySearchResult, AppError>;

    /// Inspects exactly one concrete instance.
    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<Resolution<ItemInspection>, AppError>;
}

impl<G, R> QueryOperations for QueryService<G, R>
where
    G: ProfileGateway,
    R: InventoryRepository
        + SearchRepository
        + DefinitionRepository
        + destiny_core::StatDefinitionRepository,
{
    fn character_subclasses(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassResult, AppError> {
        QueryService::character_subclasses(self, character_id, policy, now)
    }

    fn list_characters(
        &mut self,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<CharacterListResult, AppError> {
        QueryService::list_characters(self, policy, now)
    }

    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<InventorySearchResult, AppError> {
        QueryService::search_inventory(self, request, policy, now)
    }

    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<Resolution<ItemInspection>, AppError> {
        QueryService::inspect_item(self, request, policy, now)
    }
}

/// The sanitized error for a projection that cannot answer consistently.
fn invalid_repository_state() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// Builds the committed-post-sync metadata for one query answer.
///
/// The profile revision, `checked_at`, `source`, and `definition_version` come
/// from the repository-reported snapshot after synchronization; the freshness
/// status is evaluated at the caller-supplied `now`, advanced to at least the
/// refreshed `checked_at` only when acquisition occurred. A skipped read keeps
/// the original time and the core's future-timestamp guard. The profile
/// projection and the Manifest definition projection stay distinct.
fn query_metadata(
    outcome: &ProfileSyncOutcome,
    policy: FreshnessPolicy,
    now: Timestamp,
) -> QueryMetadata {
    let metadata = outcome.metadata();
    let now = match outcome {
        ProfileSyncOutcome::Refreshed { .. } => now.max(metadata.checked_at),
        ProfileSyncOutcome::SkippedFresh { .. } => now,
    };
    let freshness = FreshnessInput {
        dataset: Dataset::ProfileInventory,
        checked_at: Some(metadata.checked_at),
        source: Some(metadata.source),
        now,
        policy,
    }
    .status();
    QueryMetadata {
        sync: SyncMetadata {
            dataset: Dataset::ProfileInventory,
            checked_at: metadata.checked_at,
            source: metadata.source,
            revision: metadata.revision,
            definition_version: metadata.definition_version.clone(),
        },
        freshness,
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::{QueryOperations, QueryService, SearchInventoryRequest};
    use crate::sync::ProfileSyncService;
    use crate::test_support::{
        InMemoryDefinitionRepository, InMemoryInventoryRepository, InMemorySearchRepository,
        QueuedProfileGateway,
    };
    use destiny_core::{
        AppError, Character, CharacterId, CharacterListResult, Dataset, DefinitionReplacementInput,
        DefinitionRepository, DefinitionSearchQuery, DefinitionVersion, DestinyMembership,
        ErrorCategory, FreshnessPolicy, FreshnessStatus, InspectItemRequest, InstanceSearchQuery,
        InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemDefinition, ItemHash,
        ItemHashFilter, ItemInstance, ItemInstanceId, ItemLocation, MAX_SEARCH_LIMIT, MembershipId,
        MembershipType, PerkHash, PerkSummary, ProfileGateway, Resolution, SearchRepository,
        SearchTerm, SourceMarker, SyncMetadata, SyncRevision, Timestamp,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(10_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    const DEFINITION_VERSION: &str = "definitions-v1";
    const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

    /// A narrow test-local composite exposing one repository for all three seams.
    ///
    /// The three in-memory fakes model independent projection tables. The
    /// composite keeps them coherent: committing a profile refresh also updates
    /// the instance search projection, so a query after a refresh observes the
    /// new instances. It records every read-seam call so tests can prove the
    /// exact sync-then-query ordering and that a failed refresh issues no query.
    #[derive(Clone, Debug, Default)]
    struct CompositeRepository {
        inventory: InMemoryInventoryRepository,
        search: InMemorySearchRepository,
        definitions: InMemoryDefinitionRepository,
        calls: RefCell<Vec<&'static str>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            let version = DefinitionVersion::new(DEFINITION_VERSION);
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions.clone()),
                definitions: InMemoryDefinitionRepository::new(version, definitions),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        fn recorded_calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }

        /// Commits one valid profile projection and mirrors its instances.
        fn commit(&mut self, input: InventorySnapshotInput) {
            let snapshot = self
                .inventory
                .replace_current(input)
                .expect("valid seed snapshot");
            self.search.set_instances(snapshot.items);
        }

        /// Commits one seeded projection at an explicit revision, mirroring it.
        fn commit_with_revision(&mut self, input: InventorySnapshotInput, revision: SyncRevision) {
            self.inventory = InMemoryInventoryRepository::with_revision_for_test(input, revision);
            let snapshot = self
                .inventory
                .load_current()
                .expect("repository read")
                .expect("seeded snapshot");
            self.search.set_instances(snapshot.items);
        }
    }

    impl InventoryRepository for CompositeRepository {
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.record("load_current");
            self.inventory.load_current()
        }

        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.record("replace_current");
            let snapshot = self.inventory.replace_current(input)?;
            self.search.set_instances(snapshot.items.clone());
            Ok(snapshot)
        }
    }

    impl SearchRepository for CompositeRepository {
        fn name_catalog(
            &self,
            kind: &str,
        ) -> Result<destiny_core::name_matching::NameCatalog, AppError> {
            if kind == "stat" {
                Ok(self.definitions.stat_name_catalog())
            } else {
                self.search.name_catalog(kind)
            }
        }
        fn search_definitions(
            &self,
            query: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.record("search_definitions");
            self.search.search_definitions(query)
        }

        fn search_instances(
            &self,
            query: &InstanceSearchQuery,
        ) -> Result<Vec<ItemInstance>, AppError> {
            self.record("search_instances");
            self.search.search_instances(query)
        }

        fn lookup_instance(
            &self,
            instance_id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.record("lookup_instance");
            self.search.lookup_instance(instance_id)
        }
    }

    impl DefinitionRepository for CompositeRepository {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.definitions.current_definition_version()
        }

        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<ItemDefinition>, AppError> {
            self.record("lookup_definition");
            self.definitions.lookup_definition(hash)
        }

        fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
            self.definitions.current_metadata()
        }

        fn replace_definitions(
            &mut self,
            replacement: DefinitionReplacementInput,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.replace_definitions(replacement)
        }
    }

    impl destiny_core::StatDefinitionRepository for CompositeRepository {
        fn lookup_stat(&self, hash: u32) -> Result<Option<destiny_core::StatDefinition>, AppError> {
            self.definitions.lookup_stat(hash)
        }
        fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
            self.definitions.stat_hashes_by_name(name)
        }
        fn stat_definition_count(&self) -> Result<usize, AppError> {
            self.definitions.stat_definition_count()
        }
    }

    /// A read-only spy over the fake gateway the sync service owns by value.
    struct SharedGateway(Rc<RefCell<QueuedProfileGateway>>);

    impl ProfileGateway for SharedGateway {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.0.borrow_mut().resolve_membership()
        }

        fn load_inventory(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.borrow_mut().load_inventory(membership)
        }
    }

    type Service = QueryService<SharedGateway, CompositeRepository>;

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
    }

    fn service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        repository: CompositeRepository,
    ) -> Service {
        QueryService::new(ProfileSyncService::new(
            SharedGateway(Rc::clone(shared)),
            repository,
        ))
    }

    fn membership() -> DestinyMembership {
        DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new(MEMBERSHIP_IDENTIFIER),
        )
        .expect("valid membership")
    }

    fn character(id: &str) -> Character {
        Character {
            id: CharacterId::new(id),
            name: Some(format!("name-{id}")),
            class: None,
            race: None,
            gender: None,
            light: None,
        }
    }

    fn definition(
        hash: i64,
        name: &str,
        item_type: Option<&str>,
        tags: Vec<&str>,
    ) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: item_type.map(str::to_string),
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            version: DefinitionVersion::new(DEFINITION_VERSION),
        }
    }

    fn standard_definitions() -> Vec<ItemDefinition> {
        vec![
            definition(
                1,
                "Gjallarhorn",
                Some("Rocket Launcher"),
                vec!["exotic", "solar"],
            ),
            definition(
                2,
                "Fatebringer",
                Some("Hand Cannon"),
                vec!["legendary", "kinetic"],
            ),
            definition(
                3,
                "Vex Mythoclast",
                Some("Fusion Rifle"),
                vec!["exotic", "solar"],
            ),
        ]
    }

    fn perk(hash: u32) -> PerkSummary {
        PerkSummary {
            perk_hash: PerkHash::new(hash),
            active: true,
            visible: true,
        }
    }

    fn instance(
        instance_id: &str,
        item_hash: i64,
        location: ItemLocation,
        perks: Vec<PerkSummary>,
    ) -> ItemInstance {
        ItemInstance {
            available_plugs: Vec::new(),
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            stats: Vec::new(),
            plugs: Vec::new(),
            perks,
        }
    }

    fn snapshot_input(
        checked_at: Timestamp,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> InventorySnapshotInput {
        let membership = membership();
        InventorySnapshotInput {
            membership_id: membership.membership_id().clone(),
            membership_type: membership.membership_type(),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at,
            source: SourceMarker::Profile,
            characters,
            items,
        }
    }

    fn fresh_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - 100)
    }

    fn stale_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - TTL.ttl_seconds - 1)
    }

    fn queue_refresh(shared: &Rc<RefCell<QueuedProfileGateway>>, input: InventorySnapshotInput) {
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared.borrow_mut().queue_inventory(Ok(input));
    }

    fn definition_query(
        text: Option<&str>,
        item_type: Option<&str>,
        tags: Vec<&str>,
        limit: usize,
    ) -> DefinitionSearchQuery {
        let term = |value: &str| SearchTerm::new(value).expect("valid term");
        DefinitionSearchQuery {
            tier: None,
            text: text.map(term),
            item_type: item_type.map(term),
            tags: tags.into_iter().map(term).collect(),
            element: None,
            slot: None,
            class_type: None,
            limit,
        }
    }

    fn instance_query(
        item_hashes: ItemHashFilter,
        perk_hash: Option<PerkHash>,
        character_id: Option<CharacterId>,
        limit: usize,
    ) -> InstanceSearchQuery {
        InstanceSearchQuery {
            sort: None,
            location: None,
            equipped: None,
            include_available: false,
            item_hashes,
            perk_hash,
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: Vec::new(),
            character_id,
            definitions: None,
            min_power: None,
            limit,
        }
    }

    fn search_request(
        definitions: DefinitionSearchQuery,
        instances: Option<InstanceSearchQuery>,
    ) -> SearchInventoryRequest {
        SearchInventoryRequest {
            definitions,
            instances,
        }
    }

    fn fresh_service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> Service {
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(fresh_checked_at(), characters, items));
        service(shared, repository)
    }

    #[test]
    fn character_subclasses_resolves_ordered_slots_and_selection() {
        use destiny_core::{BucketHash, PlugSummary, SubclassPlugKind};
        let shared = gateway();
        let mut subclass = definition(10, "Prismatic", None, vec![]);
        subclass.bucket_hash = Some(BucketHash::new(3284755031));
        subclass.element = Some("Prismatic".into());
        let cases = [
            (20, "Super", "hunter.prism.supers", SubclassPlugKind::Super),
            (
                21,
                "Aspect",
                "hunter.prism.aspects",
                SubclassPlugKind::Aspect,
            ),
            (
                22,
                "Fragment",
                "shared.fragments",
                SubclassPlugKind::Fragment,
            ),
            (
                23,
                "Empty Fragment Socket",
                "shared.fragments",
                SubclassPlugKind::Fragment,
            ),
            (
                24,
                "Grenade",
                "hunter.prism.prism_grenade",
                SubclassPlugKind::Grenade,
            ),
            (25, "Melee", "hunter.prism.melee", SubclassPlugKind::Melee),
            (
                26,
                "Dodge",
                "hunter.prism.class_abilities",
                SubclassPlugKind::ClassAbility,
            ),
            (
                27,
                "Jump",
                "hunter.prism.movement",
                SubclassPlugKind::Movement,
            ),
            (
                28,
                "Transcendence",
                "hunter.prism.transcendence",
                SubclassPlugKind::Transcendence,
            ),
        ];
        let mut definitions = vec![subclass, definition(30, "Shader", None, vec![])];
        for (hash, name, category, _) in &cases {
            let mut plug = definition(*hash, name, None, vec![]);
            plug.plug_category = Some((*category).into());
            definitions.push(plug);
        }
        let id = CharacterId::new("a");
        let mut hunter = character("a");
        hunter.class = Some("Hunter".into());
        let mut equipped = instance("subclass", 10, ItemLocation::Character(id.clone()), vec![]);
        equipped.equipped = true;
        equipped.plugs = [20, 21, 99, 22, 30, 23, 24, 25, 26, 27, 28]
            .into_iter()
            .map(|hash| PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        let mut unequipped = equipped.clone();
        unequipped.instance_id = "unequipped".into();
        unequipped.equipped = false;
        let mut wrong_bucket = equipped.clone();
        wrong_bucket.instance_id = "other".into();
        wrong_bucket.item_hash = ItemHash::new(30);
        let mut repository = CompositeRepository::with_definitions(definitions);
        repository.commit(snapshot_input(
            fresh_checked_at(),
            vec![hunter, character("b")],
            vec![equipped, unequipped, wrong_bucket],
        ));
        let mut service = service(&shared, repository);
        let result = service.character_subclasses(None, TTL, NOW).unwrap();
        assert_eq!(result.characters.len(), 2);
        assert_eq!(result.characters[0].class.as_deref(), Some("Hunter"));
        assert!(result.characters[1].subclass.is_none());
        let config = result.characters[0].subclass.as_ref().unwrap();
        assert_eq!(config.instance_id.as_str(), "subclass");
        assert_eq!(config.item_hash, ItemHash::new(10));
        assert_eq!(config.name, "Prismatic");
        assert_eq!(config.element.as_deref(), Some("Prismatic"));
        assert_eq!(config.slots.len(), cases.len());
        for (slot, (hash, name, _, kind)) in config.slots.iter().zip(cases) {
            assert_eq!(slot.kind, kind);
            assert_eq!(slot.plug_hash, ItemHash::new(hash));
            assert_eq!(slot.name.as_deref(), Some(name));
            assert_eq!(slot.empty, hash == 23);
        }
        let selected = service
            .character_subclasses(Some(&CharacterId::new("hUnTeR")), TTL, NOW)
            .unwrap();
        assert_eq!(selected.characters, vec![result.characters[0].clone()]);
        let selected = service
            .character_subclasses(Some(&CharacterId::new("b")), TTL, NOW)
            .unwrap();
        assert_eq!(selected.characters, vec![result.characters[1].clone()]);
        assert!(
            service
                .character_subclasses(Some(&CharacterId::new("missing")), TTL, NOW)
                .is_err()
        );
        assert_eq!(shared.borrow().membership_calls(), 0);
    }

    #[test]
    fn character_subclasses_requires_definitions_and_refreshes_before_reading() {
        let shared = gateway();
        let mut service = service(&shared, CompositeRepository::default());
        assert_eq!(
            service
                .character_subclasses(None, TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::DefinitionsUnavailable
        );
        assert_eq!(shared.borrow().membership_calls(), 0);
        service.repository_mut().definitions = InMemoryDefinitionRepository::new(
            DefinitionVersion::new(DEFINITION_VERSION),
            standard_definitions(),
        );
        queue_refresh(&shared, snapshot_input(NOW, vec![character("new")], vec![]));
        let result = service.character_subclasses(None, TTL, NOW).unwrap();
        assert_eq!(result.characters[0].character_id, CharacterId::new("new"));
        assert!(result.characters[0].subclass.is_none());
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(
            service.repository().recorded_calls().last(),
            Some(&"load_current")
        );
    }

    #[test]
    fn character_subclasses_rejects_duplicate_equipped_items_and_ambiguous_classes() {
        let shared = gateway();
        let mut subclass = definition(10, "Prismatic", None, vec![]);
        subclass.bucket_hash = Some(destiny_core::SUBCLASS_BUCKET_HASH);
        let mut repository = CompositeRepository::with_definitions(vec![subclass]);
        let mut hunter = character("a");
        hunter.class = Some("Hunter".into());
        let mut other = character("b");
        other.class = hunter.class.clone();
        let mut equipped = instance(
            "one",
            10,
            ItemLocation::Character(hunter.id.clone()),
            vec![],
        );
        equipped.equipped = true;
        let mut duplicate = equipped.clone();
        duplicate.instance_id = "two".into();
        repository.commit(snapshot_input(
            fresh_checked_at(),
            vec![hunter, other],
            vec![equipped, duplicate],
        ));
        let mut service = service(&shared, repository);
        assert_eq!(
            service
                .character_subclasses(None, TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::LocalStorage
        );
        assert_eq!(
            service
                .character_subclasses(Some(&CharacterId::new("Hunter")), TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::AmbiguousMatch
        );
        assert!(
            service
                .character_subclasses(Some(&CharacterId::new("b")), TTL, NOW)
                .unwrap()
                .characters[0]
                .subclass
                .is_none()
        );
    }

    #[test]
    fn perk_search_resolves_names_in_socket_order_and_omits_unknowns() {
        let shared = gateway();
        let mut rampage = definition(60, "Rampage", None, vec![]);
        rampage.plug_category = Some("frames".into());
        let mut barrel = definition(61, "Barrel", None, vec![]);
        barrel.plug_category = Some("barrels".into());
        let mut cosmetic = definition(62, "Shader", None, vec![]);
        cosmetic.plug_category = Some("shader".into());
        let mut definitions = standard_definitions();
        definitions
            .iter_mut()
            .find(|definition| definition.hash == ItemHash::new(1))
            .unwrap()
            .slot = Some(destiny_core::domain::ItemSlot::Kinetic);
        let mut alternative = definition(63, "Kill Clip", None, vec![]);
        alternative.plug_category = Some("frames".into());
        definitions.extend([rampage, barrel, cosmetic, alternative]);
        let mut repository = CompositeRepository::with_definitions(definitions);
        let mut item = instance("owned", 1, ItemLocation::Vault, vec![]);
        item.available_plugs = [60, 63, 99]
            .into_iter()
            .map(|hash| destiny_core::AvailablePlug {
                socket_index: 0,
                plug_hash: ItemHash::new(hash),
            })
            .collect();
        item.plugs = [61, 99, 60, 62, 60]
            .into_iter()
            .map(|hash| destiny_core::PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        repository.commit(snapshot_input(fresh_checked_at(), vec![], vec![item]));
        let mut service = service(&shared, repository);
        let mut query = instance_query(ItemHashFilter::Any, None, None, 1);
        query.perk_name = Some(vec![SearchTerm::new("rampage").unwrap()]);
        let request = search_request(definition_query(None, None, vec![], 1), Some(query));
        let result = service.search_inventory(&request, TTL, NOW).unwrap();
        assert_eq!(result.hits[0].perk_names, ["Barrel", "Rampage", "Rampage"]);
        let Resolution::Unique(inspected) = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: "owned".into(),
                },
                TTL,
                NOW,
            )
            .unwrap()
        else {
            panic!("missing inspection")
        };
        assert_eq!(inspected.perk_names, result.hits[0].perk_names);
        assert_eq!(inspected.available_perks, result.hits[0].available_perks);
        assert_eq!(inspected.available_perks, ["Kill Clip"]);
        assert!(inspected.mod_names.is_empty());
    }

    #[test]
    fn armor_search_exposes_mod_names_but_not_exotic_intrinsics() {
        let shared = gateway();
        let mut armor_mod = definition(60, "Harmonic Siphon", None, vec![]);
        armor_mod.plug_category = Some("enhancements.v2_head".into());
        let mut intrinsic = definition(61, "Exotic intrinsic", None, vec![]);
        intrinsic.plug_category = Some("intrinsics".into());
        let mut definitions = standard_definitions();
        definitions
            .iter_mut()
            .find(|definition| definition.hash == ItemHash::new(1))
            .unwrap()
            .slot = Some(destiny_core::domain::ItemSlot::Helmet);
        let mut archetype = definition(62, "Specialist", None, vec![]);
        archetype.plug_category = Some("armor_archetypes".into());
        let mut second = definition(63, "Paragon", None, vec![]);
        second.plug_category = Some("armor_archetypes".into());
        definitions.extend([armor_mod, intrinsic, archetype, second]);
        let mut repository = CompositeRepository::with_definitions(definitions);
        let mut item = instance("owned", 1, ItemLocation::Vault, vec![]);
        item.plugs = [61, 60, 99, 62, 63, 60]
            .into_iter()
            .map(|hash| destiny_core::PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        repository.commit(snapshot_input(fresh_checked_at(), vec![], vec![item]));
        let mut service = service(&shared, repository);
        let mut query = instance_query(ItemHashFilter::Any, None, None, 1);
        query.mod_name = Some(vec![SearchTerm::new(" harmonic  siphon ").unwrap()]);
        query.archetype = Some(SearchTerm::new("specialist").unwrap());
        let request = search_request(definition_query(None, None, vec![], 1), Some(query));
        let result = service.search_inventory(&request, TTL, NOW).unwrap();
        assert_eq!(
            result.hits[0].mod_names,
            ["Harmonic Siphon", "Harmonic Siphon"]
        );
        assert!(result.hits[0].perk_names.is_empty());
        assert_eq!(result.hits[0].archetype.as_deref(), Some("Specialist"));
        let Resolution::Unique(inspected) = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: "owned".into(),
                },
                TTL,
                NOW,
            )
            .unwrap()
        else {
            panic!("missing inspection")
        };
        assert_eq!(inspected.mod_names, result.hits[0].mod_names);
        assert_eq!(inspected.archetype, result.hits[0].archetype);
        assert_eq!(inspected.available_mods, result.hits[0].available_mods);
    }

    #[test]
    fn absent_manifest_is_distinct_from_loaded_catalog_without_matches() {
        let shared = gateway();
        let mut empty = service(&shared, CompositeRepository::default());
        let request = search_request(
            DefinitionSearchQuery {
                tier: None,
                text: Some(SearchTerm::new("no such item").unwrap()),
                item_type: None,
                tags: Vec::new(),
                element: None,
                slot: None,
                class_type: None,
                limit: 5,
            },
            None,
        );
        let error = empty.search_inventory(&request, TTL, NOW).unwrap_err();
        assert_eq!(
            error.to_string(),
            "definitions not loaded; run ghostlink sync-manifest"
        );
        let mut loaded = fresh_service(&shared, Vec::new(), Vec::new());
        assert!(
            loaded
                .search_inventory(&request, TTL, NOW)
                .unwrap()
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn manifest_metadata_failure_is_not_reported_as_an_absent_catalog() {
        let shared = gateway();
        let repository = CompositeRepository {
            definitions: InMemoryDefinitionRepository::default()
                .with_metadata_failure(AppError::new(ErrorCategory::LocalStorage)),
            ..CompositeRepository::default()
        };
        let mut service = service(&shared, repository);
        let request = search_request(
            DefinitionSearchQuery {
                tier: None,
                text: None,
                item_type: None,
                tags: Vec::new(),
                element: None,
                slot: None,
                class_type: None,
                limit: 5,
            },
            None,
        );
        assert_eq!(
            service
                .search_inventory(&request, TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::LocalStorage
        );
    }

    #[test]
    fn query_operations_trait_dispatches_to_the_shared_service() {
        let shared = gateway();
        let mut service = fresh_service(&shared, vec![character("warlock")], Vec::new());

        // Naming the trait object type is a type-level proof that the surface
        // is object-safe. The generic helper then proves it delegates to the
        // shared implementation, preserving committed metadata and the skip.
        let _object_safe: Option<&mut dyn QueryOperations> = None;
        fn list_characters<S: QueryOperations + ?Sized>(
            service: &mut S,
        ) -> Result<CharacterListResult, AppError> {
            service.list_characters(TTL, NOW)
        }

        let listed = list_characters(&mut service).expect("trait list succeeds");

        assert_eq!(listed.characters.len(), 1);
        assert_eq!(listed.characters[0].id.as_str(), "warlock");
        assert_eq!(listed.metadata.sync.dataset, Dataset::ProfileInventory);
        assert_eq!(listed.metadata.sync.source, SourceMarker::Profile);
        assert_eq!(listed.metadata.sync.revision, SyncRevision::new(1));
        assert_eq!(listed.metadata.freshness, FreshnessStatus::Fresh);
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn fresh_ordinary_list_characters_skips_the_gateway() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            vec![character("warlock"), character("hunter")],
            Vec::new(),
        );

        let result = service
            .list_characters(TTL, NOW)
            .expect("fresh read succeeds");

        assert_eq!(result.characters.len(), 2);
        assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
        assert_eq!(result.metadata.sync.dataset, Dataset::ProfileInventory);
        assert_eq!(result.metadata.sync.source, SourceMarker::Profile);
        assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
        assert_eq!(result.metadata.sync.checked_at, fresh_checked_at());
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn refreshed_reads_are_fresh_when_gateway_checked_at_is_after_query_now() {
        // The injected query time is read before acquisition; the fake gateway
        // publishes a snapshot stamped one second later, as the live gateway can.
        let checked_at = Timestamp::new(NOW.seconds() + 1);
        for path in ["list", "search", "inspect"] {
            let shared = gateway();
            queue_refresh(
                &shared,
                snapshot_input(
                    checked_at,
                    vec![character("hunter")],
                    vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
                ),
            );
            let mut service = service(
                &shared,
                CompositeRepository::with_definitions(standard_definitions()),
            );
            let metadata = match path {
                "list" => service.list_characters(TTL, NOW).expect("list").metadata,
                "search" => {
                    service
                        .search_inventory(
                            &search_request(
                                definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                                None,
                            ),
                            TTL,
                            NOW,
                        )
                        .expect("search")
                        .metadata
                }
                "inspect" => match service
                    .inspect_item(
                        &InspectItemRequest {
                            instance_id: ItemInstanceId::new("inst-a"),
                        },
                        TTL,
                        NOW,
                    )
                    .expect("inspect")
                {
                    Resolution::Unique(inspection) => inspection.metadata,
                    other => panic!("expected inspection, got {other:?}"),
                },
                _ => unreachable!(),
            };
            assert_eq!(shared.borrow().inventory_calls(), 1, "{path}");
            assert_eq!(metadata.sync.checked_at, checked_at, "{path}");
            assert_eq!(metadata.freshness, FreshnessStatus::Fresh, "{path}");
        }
    }

    #[test]
    fn post_sync_freshness_preserves_time_and_source_guards() {
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            Timestamp::new(NOW.seconds() + 1),
            Vec::new(),
            Vec::new(),
        ));
        let metadata = repository
            .load_current()
            .expect("read")
            .expect("snapshot")
            .metadata;
        let skipped = destiny_core::ProfileSyncOutcome::SkippedFresh {
            metadata: metadata.clone(),
        };
        assert_eq!(
            super::query_metadata(&skipped, TTL, NOW).freshness,
            FreshnessStatus::Stale,
            "no refresh must not advance the caller's time",
        );
        let mut old = metadata.clone();
        old.checked_at = stale_checked_at();
        let refreshed = destiny_core::ProfileSyncOutcome::Refreshed { metadata: old };
        assert_eq!(
            super::query_metadata(&refreshed, TTL, NOW).freshness,
            FreshnessStatus::Stale,
            "an old refreshed timestamp must still expire",
        );
        let mut invalid = metadata;
        invalid.source = SourceMarker::Manifest;
        let refreshed = destiny_core::ProfileSyncOutcome::Refreshed { metadata: invalid };
        assert_eq!(
            super::query_metadata(&refreshed, TTL, NOW).freshness,
            FreshnessStatus::InvalidSource,
        );
    }

    #[test]
    fn missing_projection_refreshes_before_searching() {
        let shared = gateway();
        queue_refresh(
            &shared,
            snapshot_input(
                NOW,
                Vec::new(),
                vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
            ),
        );
        let mut service = service(
            &shared,
            CompositeRepository::with_definitions(standard_definitions()),
        );

        let result = service
            .search_inventory(
                &search_request(
                    definition_query(Some("gjallar"), None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("refresh then search succeeds");

        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
        assert_eq!(result.metadata.sync.checked_at, NOW);
        assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].hash, ItemHash::new(1));
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].instance.instance_id.as_str(), "inst-a");
        assert_eq!(result.hits[0].definition.hash, ItemHash::new(1));
    }

    #[test]
    fn stale_projection_refreshes_and_replaces_the_visible_instances() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            stale_checked_at(),
            Vec::new(),
            vec![instance("stale", 1, ItemLocation::Vault, Vec::new())],
        ));
        queue_refresh(
            &shared,
            snapshot_input(
                NOW,
                Vec::new(),
                vec![instance("current", 2, ItemLocation::Vault, Vec::new())],
            ),
        );
        let mut service = service(&shared, repository);

        let result = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("stale refresh then search succeeds");

        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(result.metadata.sync.revision, SyncRevision::new(2));
        assert_eq!(result.metadata.sync.checked_at, NOW);
        assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].instance.instance_id.as_str(), "current");
        assert_eq!(result.hits[0].definition.hash, ItemHash::new(2));
    }

    #[test]
    fn invalid_source_projection_refreshes_before_inspection() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        let mut mis_sourced = snapshot_input(
            NOW,
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        );
        mis_sourced.source = SourceMarker::Manifest;
        repository.commit_with_revision(mis_sourced, SyncRevision::new(4));
        queue_refresh(
            &shared,
            snapshot_input(
                NOW,
                Vec::new(),
                vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
            ),
        );
        let mut service = service(&shared, repository);

        let result = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: ItemInstanceId::new("inst-a"),
                },
                TTL,
                NOW,
            )
            .expect("invalid-source refresh then inspect succeeds");

        let inspection = match result {
            Resolution::Unique(inspection) => inspection,
            other => panic!("expected a unique inspection, got {other:?}"),
        };

        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(inspection.metadata.sync.source, SourceMarker::Profile);
        assert_eq!(inspection.metadata.freshness, FreshnessStatus::Fresh);
        // The content was unchanged, so the repository retained its revision.
        assert_eq!(inspection.metadata.sync.revision, SyncRevision::new(4));
    }

    #[test]
    fn refresh_failure_preserves_state_and_issues_no_query() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        let prior = snapshot_input(
            stale_checked_at(),
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        );
        repository.commit(prior);
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        let mut service = service(&shared, repository);

        let error = service
            .search_inventory(
                &search_request(
                    definition_query(Some("gjallar"), None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect_err("refresh failure must fail the read");

        assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
        // The prior committed projection is untouched and no query ran: the
        // only recorded read is the sync service's own freshness read.
        assert_eq!(service.repository().recorded_calls(), ["load_current"]);
        let committed = service
            .repository()
            .load_current()
            .unwrap()
            .expect("prior projection preserved");
        assert_eq!(committed.metadata.revision, SyncRevision::new(1));
        assert_eq!(committed.metadata.checked_at, stale_checked_at());
    }

    #[test]
    fn character_listing_is_deterministic_in_identifier_order() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            vec![character("zeta"), character("alpha"), character("mid")],
            Vec::new(),
        );

        let result = service
            .list_characters(TTL, NOW)
            .expect("fresh read succeeds");

        let ids: Vec<&str> = result
            .characters
            .iter()
            .map(|character| character.id.as_str())
            .collect();
        assert_eq!(ids, ["alpha", "mid", "zeta"]);
    }

    #[test]
    fn definition_search_matches_name_type_and_tags() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());

        let by_name = service
            .search_inventory(
                &search_request(
                    definition_query(Some("  GJALLAR "), None, Vec::new(), MAX_SEARCH_LIMIT),
                    None,
                ),
                TTL,
                NOW,
            )
            .expect("name search succeeds");
        assert_eq!(
            by_name
                .candidates
                .iter()
                .map(|definition| definition.hash)
                .collect::<Vec<_>>(),
            [ItemHash::new(1)]
        );

        let by_type = service
            .search_inventory(
                &search_request(
                    definition_query(None, Some("hand cannon"), Vec::new(), MAX_SEARCH_LIMIT),
                    None,
                ),
                TTL,
                NOW,
            )
            .expect("type search succeeds");
        assert_eq!(by_type.candidates.len(), 1);
        assert_eq!(by_type.candidates[0].hash, ItemHash::new(2));

        let by_tags = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, vec!["exotic", "solar"], MAX_SEARCH_LIMIT),
                    None,
                ),
                TTL,
                NOW,
            )
            .expect("tag search succeeds");
        assert_eq!(
            by_tags
                .candidates
                .iter()
                .map(|definition| definition.hash)
                .collect::<Vec<_>>(),
            [ItemHash::new(1), ItemHash::new(3)]
        );
    }

    #[test]
    fn instance_search_matches_perk_and_pairs_the_exact_definition() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![
                instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)]),
                instance("inst-b", 2, ItemLocation::Vault, vec![perk(20)]),
            ],
        );

        let by_perk = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        Some(PerkHash::new(10)),
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("perk search succeeds");

        assert_eq!(by_perk.hits.len(), 1);
        assert_eq!(by_perk.hits[0].instance.instance_id.as_str(), "inst-a");
        // The hit pairs the instance with the definition its hash references.
        assert_eq!(by_perk.hits[0].definition.hash, ItemHash::new(1));
        assert_eq!(
            by_perk.hits[0].definition.hash,
            by_perk.hits[0].instance.item_hash
        );

        let by_hash = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::AnyOf(vec![ItemHash::new(2)]),
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("hash search succeeds");
        assert_eq!(by_hash.hits.len(), 1);
        assert_eq!(by_hash.hits[0].instance.instance_id.as_str(), "inst-b");
        assert_eq!(by_hash.hits[0].definition.hash, ItemHash::new(2));
    }

    #[test]
    fn instance_search_matches_character_location_and_excludes_the_vault() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            vec![character("warlock")],
            vec![
                instance(
                    "equipped",
                    1,
                    ItemLocation::Character(CharacterId::new("warlock")),
                    Vec::new(),
                ),
                instance("stored", 1, ItemLocation::Vault, Vec::new()),
            ],
        );

        let result = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        Some(CharacterId::new("warlock")),
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("character search succeeds");

        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].instance.instance_id.as_str(), "equipped");
    }

    #[test]
    fn definition_filters_constrain_owned_hits_without_an_explicit_instance_query() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![
                instance("a-other", 1, ItemLocation::Vault, Vec::new()),
                instance("z-match", 2, ItemLocation::Vault, Vec::new()),
            ],
        );
        let request = search_request(
            definition_query(None, Some("HAND CANNON"), Vec::new(), 1),
            None,
        );
        let result = service.search_inventory(&request, TTL, NOW).unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].instance.instance_id.as_str(), "z-match");
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn definition_filters_constrain_explicit_instance_query_before_limit() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![
                instance("a-other", 1, ItemLocation::Vault, vec![perk(10)]),
                instance("z-match", 2, ItemLocation::Vault, vec![perk(10)]),
            ],
        );
        let request = search_request(
            definition_query(None, Some("hand cannon"), Vec::new(), 1),
            Some(instance_query(
                ItemHashFilter::Any,
                Some(PerkHash::new(10)),
                None,
                1,
            )),
        );
        let result = service.search_inventory(&request, TTL, NOW).unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].instance.instance_id.as_str(), "z-match");
    }

    #[test]
    fn definition_only_search_skips_instance_search() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());

        let result = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    None,
                ),
                TTL,
                NOW,
            )
            .expect("definition-only search succeeds");

        assert_eq!(result.candidates.len(), 3);
        assert!(result.hits.is_empty());
        assert_eq!(
            service.repository().recorded_calls(),
            ["load_current", "search_definitions"]
        );
    }

    #[test]
    fn empty_matches_are_explicit_and_never_guess() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        );

        let no_definitions = service
            .search_inventory(
                &search_request(
                    definition_query(Some("nonexistent"), None, Vec::new(), MAX_SEARCH_LIMIT),
                    None,
                ),
                TTL,
                NOW,
            )
            .expect("no-match search succeeds");
        assert!(no_definitions.candidates.is_empty());

        let no_instances = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::AnyOf(Vec::new()),
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("empty candidate set search succeeds");
        assert!(no_instances.hits.is_empty());
    }

    #[test]
    fn exact_inspection_reports_the_definition_and_instance_separately() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)])],
        );

        let resolution = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: ItemInstanceId::new("inst-a"),
                },
                TTL,
                NOW,
            )
            .expect("inspection succeeds");

        match resolution {
            Resolution::Unique(inspection) => {
                assert_eq!(inspection.instance.instance_id.as_str(), "inst-a");
                assert_eq!(inspection.instance.item_hash, ItemHash::new(1));
                assert_eq!(inspection.definition.hash, ItemHash::new(1));
                assert_eq!(inspection.metadata.freshness, FreshnessStatus::Fresh);
                assert_eq!(inspection.metadata.sync.source, SourceMarker::Profile);
            }
            other => panic!("expected a unique inspection, got {other:?}"),
        }
    }

    #[test]
    fn inspection_of_an_unknown_instance_is_missing() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());

        let resolution = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: ItemInstanceId::new("absent"),
                },
                TTL,
                NOW,
            )
            .expect("missing inspection succeeds");

        assert_eq!(resolution, Resolution::Missing);
    }

    #[test]
    fn ambiguous_instance_identifier_fails_closed() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());
        let duplicate = instance("dup", 1, ItemLocation::Vault, Vec::new());
        service
            .repository_mut()
            .search
            .set_instances(vec![duplicate.clone(), duplicate]);

        let error = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: ItemInstanceId::new("dup"),
                },
                TTL,
                NOW,
            )
            .expect_err("ambiguous instance must fail closed");

        assert_eq!(error.category(), ErrorCategory::LocalStorage);
    }

    #[test]
    fn instance_referencing_a_missing_definition_fails_closed() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![instance("orphan", 99, ItemLocation::Vault, Vec::new())],
        );

        let search_error = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect_err("missing definition must fail closed");
        assert_eq!(search_error.category(), ErrorCategory::LocalStorage);

        let inspect_error = service
            .inspect_item(
                &InspectItemRequest {
                    instance_id: ItemInstanceId::new("orphan"),
                },
                TTL,
                NOW,
            )
            .expect_err("missing definition must fail closed");
        assert_eq!(inspect_error.category(), ErrorCategory::LocalStorage);
    }

    #[test]
    fn duplicated_definition_hashes_fail_closed() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        let version = DefinitionVersion::new(DEFINITION_VERSION);
        let duplicated = definition(1, "Gjallarhorn", Some("Rocket Launcher"), Vec::new());
        repository.definitions =
            InMemoryDefinitionRepository::new(version, vec![duplicated.clone(), duplicated]);
        repository.commit(snapshot_input(
            fresh_checked_at(),
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect_err("duplicated definition must fail closed");

        assert_eq!(error.category(), ErrorCategory::LocalStorage);
    }

    #[test]
    fn unchanged_refresh_retains_revision_and_refreshes_metadata() {
        let shared = gateway();
        let content = vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())];
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            stale_checked_at(),
            Vec::new(),
            content.clone(),
        ));
        queue_refresh(&shared, snapshot_input(NOW, Vec::new(), content));
        let mut service = service(&shared, repository);

        let result = service.list_characters(TTL, NOW).expect("refresh succeeds");

        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
        assert_eq!(result.metadata.sync.checked_at, NOW);
        assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
    }

    #[test]
    fn changed_refresh_advances_revision_once() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            stale_checked_at(),
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        ));
        queue_refresh(
            &shared,
            snapshot_input(
                NOW,
                Vec::new(),
                vec![
                    instance("inst-a", 1, ItemLocation::Vault, Vec::new()),
                    instance("inst-b", 2, ItemLocation::Vault, Vec::new()),
                ],
            ),
        );
        let mut service = service(&shared, repository);

        let result = service.list_characters(TTL, NOW).expect("refresh succeeds");

        assert_eq!(result.metadata.sync.revision, SyncRevision::new(2));
        assert_eq!(result.metadata.sync.checked_at, NOW);
        assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
    }

    #[test]
    fn definition_queries_are_validated_before_any_sync() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());

        let error = service
            .search_inventory(
                &search_request(definition_query(None, None, Vec::new(), 0), None),
                TTL,
                NOW,
            )
            .expect_err("invalid limit must be rejected");

        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert!(service.repository().recorded_calls().is_empty());
    }

    #[test]
    fn instance_queries_are_validated_before_any_sync() {
        let shared = gateway();
        let mut service = fresh_service(&shared, Vec::new(), Vec::new());
        let over_large: Vec<ItemHash> = (0..=64).map(ItemHash::new).collect();

        let error = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::AnyOf(over_large),
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect_err("over-large hash filter must be rejected");

        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert!(service.repository().recorded_calls().is_empty());
    }

    #[test]
    fn search_ordering_is_sync_then_definitions_then_instances_then_definition() {
        let shared = gateway();
        let mut service = fresh_service(
            &shared,
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, Vec::new())],
        );

        service
            .search_inventory(
                &search_request(
                    definition_query(Some("gjallar"), None, Vec::new(), MAX_SEARCH_LIMIT),
                    Some(instance_query(
                        ItemHashFilter::Any,
                        None,
                        None,
                        MAX_SEARCH_LIMIT,
                    )),
                ),
                TTL,
                NOW,
            )
            .expect("search succeeds");

        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "search_definitions",
                "search_instances",
                "lookup_definition",
            ]
        );
    }

    #[test]
    fn refresh_failures_are_sanitized() {
        const PAYLOAD: &str = "raw-provider-payload-sentinel";
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            stale_checked_at(),
            Vec::new(),
            vec![instance(PAYLOAD, 1, ItemLocation::Vault, Vec::new())],
        ));
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderValidation)));
        let mut service = service(&shared, repository);

        let error = service
            .list_characters(TTL, NOW)
            .expect_err("refresh failure must fail the read");

        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(
                !rendered.contains(MEMBERSHIP_IDENTIFIER),
                "leaked membership identifier: {rendered}"
            );
            assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
        }
    }
}
