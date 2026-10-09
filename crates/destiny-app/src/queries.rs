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

    /// Project socketed intrinsics and plugs admitted by inspection name fields.
    /// Alternatives use the same normalized-name dedupe as inventory search;
    /// repeated equipped sockets are collapsed by hash/role only in this list.
    fn inspect_details(
        &self,
        repository: &impl destiny_core::ports::WorldDefinitionRepository,
        hit: &InventorySearchHit,
    ) -> Result<Vec<destiny_core::InspectionPlug>, AppError> {
        use destiny_core::{InspectionPlug, InspectionPlugRole as Role};
        let mut selected = Vec::new();
        for hash in hit.instance.plugs.iter().map(|plug| plug.plug_hash) {
            let Some(Some(plug)) = self.plugs.get(&hash) else {
                continue;
            };
            if plug.plug_category.as_deref() == Some("intrinsics") {
                selected.push((plug, Role::Intrinsic));
                continue;
            }
            let (perks, mods) = destiny_core::query::inventory_plug_names(
                &hit.definition,
                std::slice::from_ref(plug),
            );
            if !perks.is_empty() {
                selected.push((plug, Role::Perk));
            }
            if !mods.is_empty() {
                selected.push((plug, Role::Mod));
            }
        }
        if hit.archetype.is_some()
            && let Some(plug) = hit
                .instance
                .plugs
                .iter()
                .filter_map(|plug| self.plugs.get(&plug.plug_hash).and_then(Option::as_ref))
                .find(|plug| destiny_core::query::is_armor_archetype(plug))
        {
            selected.push((plug, Role::Archetype));
        }
        let mut available_seen = std::collections::HashSet::new();
        for hash in hit
            .instance
            .available_plugs
            .iter()
            .map(|plug| plug.plug_hash)
        {
            let Some(Some(plug)) = self.plugs.get(&hash) else {
                continue;
            };
            let (perks, mods) = destiny_core::query::inventory_plug_names(
                &hit.definition,
                std::slice::from_ref(plug),
            );
            for (names, surfaced, role) in [
                (perks, &hit.available_perks, Role::AvailablePerk),
                (mods, &hit.available_mods, Role::AvailableMod),
            ] {
                if !names.is_empty()
                    && surfaced.contains(&plug.name)
                    && available_seen
                        .insert((destiny_core::query::normalize_search_text(&plug.name), role))
                {
                    selected.push((plug, role));
                }
            }
        }
        // Stable partition: item identity first, preserving all other detail order.
        selected.sort_by_key(|(_, role)| *role != Role::Intrinsic);
        let mut seen = std::collections::HashSet::new();
        let mut descriptions = std::collections::HashMap::new();
        let mut clarity_cache = crate::clarity::ClarityCache::default();
        let mut details = Vec::new();
        for (plug, role) in selected {
            if !seen.insert((plug.hash, role)) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = descriptions.entry(plug.hash)
            {
                entry.insert(repository.lookup_item_description(plug.hash)?);
            }
            let (clarity, clarity_match) =
                clarity_cache.resolve(repository, plug, Some(hit.definition.hash))?;
            details.push(InspectionPlug {
                clarity,
                clarity_match,
                hash: plug.hash,
                name: plug.name.clone(),
                role,
                description: descriptions[&plug.hash].clone(),
            });
        }
        Ok(details)
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
/// - `query` is ANDed with both structured shapes. When present without an
///   instance query, it searches owned items using the definition query's limit;
///   an empty typed query therefore means all owned items.
/// - With neither definition filters, an instance query, nor a typed query,
///   `hits` is empty.
///
/// The service validates both queries before it refreshes anything.
///
/// [`ItemInstanceId`]: destiny_core::ItemInstanceId
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchInventoryRequest {
    pub definitions: DefinitionSearchQuery,
    pub instances: Option<InstanceSearchQuery>,
    /// Typed DIM-style predicate, ANDed with structured criteria.
    pub query: Option<destiny_core::search_query::TypedQuery>,
}

impl SearchInventoryRequest {
    pub fn new(definitions: DefinitionSearchQuery, instances: Option<InstanceSearchQuery>) -> Self {
        Self {
            definitions,
            instances,
            query: None,
        }
    }

    /// Keep the rich, spanned compiler error available to transport callers.
    pub fn with_query(
        mut self,
        input: &str,
    ) -> Result<Self, destiny_core::search_query::QueryParseError> {
        self.query = Some(destiny_core::search_query::compile(input)?);
        Ok(self)
    }
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

    /// Reads owned Postmaster holdings from the synchronized committed snapshot.
    ///
    /// Selector text is bounded and validated before synchronization. Exact ID
    /// takes priority over a unique case-insensitive class name; no selector
    /// includes all owned characters, even those with no observed holdings.
    ///
    /// A committed definition source is required only when the selected groups
    /// contain holdings to enrich. Its metadata read errors always propagate,
    /// even for empty groups. Missing individual definitions retain nameless
    /// rows; ambiguous definitions fail closed. No runtime bucket is inferred
    /// for instances. Currency coverage qualifies only component 103, not the
    /// completeness of Postmaster acquisition, including on legacy snapshots.
    /// Refresh failures propagate without serving a stale fallback.
    pub fn postmaster(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::PostmasterResult, AppError> {
        use destiny_core::{
            CharacterPostmaster, ItemLocation, PostmasterInstance, PostmasterResult,
            PostmasterStack, StackContainer,
        };
        if character_id.is_some_and(|selector| {
            let text = selector.as_str();
            text.trim().is_empty()
                || text.chars().count() > destiny_core::MAX_SEARCH_TEXT_LENGTH
                || text.chars().any(char::is_control)
        }) {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let outcome = self.synchronize(policy, now)?;
        let metadata = query_metadata(&outcome, policy, now);
        let snapshot = self.committed_snapshot()?;
        // Repository implementations normally enforce this at publication.
        // Still reject contradictory committed state, including unowned or
        // equipped Postmaster instances, rather than silently filtering it.
        if !destiny_core::InventorySnapshotInput::with_metadata(&snapshot).is_structurally_valid() {
            return Err(invalid_repository_state());
        }
        let mut characters = match character_id {
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
            None => snapshot.characters.clone(),
        };
        characters.sort_by(|left, right| left.id.cmp(&right.id));
        let has_holdings = characters.iter().any(|character| {
            snapshot
                .items
                .iter()
                .any(|item| item.location == ItemLocation::Postmaster(character.id.clone()))
                || snapshot.stacks.iter().any(|stack| {
                    stack.container() == &StackContainer::Postmaster(character.id.clone())
                })
        });
        if self.repository().current_metadata()?.is_none() && has_holdings {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        let mut names = std::collections::HashMap::new();
        let mut name_for = |hash| -> Result<Option<String>, AppError> {
            if let std::collections::hash_map::Entry::Vacant(entry) = names.entry(hash) {
                let name = match self.repository().lookup_definition(hash)? {
                    Resolution::Unique(definition) if definition.hash == hash => {
                        Some(definition.name)
                    }
                    Resolution::Missing => None,
                    Resolution::Unique(_) | Resolution::Ambiguous(_) => {
                        return Err(invalid_repository_state());
                    }
                };
                entry.insert(name);
            }
            Ok(names[&hash].clone())
        };
        let mut groups = Vec::with_capacity(characters.len());
        for character in characters {
            let mut instances = Vec::new();
            for item in snapshot
                .items
                .iter()
                .filter(|item| item.location == ItemLocation::Postmaster(character.id.clone()))
            {
                instances.push(PostmasterInstance {
                    instance_id: item.instance_id.clone(),
                    item_hash: item.item_hash,
                    name: name_for(item.item_hash)?,
                    quantity: item.quantity,
                });
            }
            instances.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
            let mut stacks = Vec::new();
            for stack in snapshot.stacks.iter().filter(|stack| {
                stack.container() == &StackContainer::Postmaster(character.id.clone())
            }) {
                stacks.push(PostmasterStack {
                    item_hash: stack.item_hash(),
                    name: name_for(stack.item_hash())?,
                    bucket_hash: stack.bucket_hash(),
                    quantity: stack.quantity(),
                });
            }
            stacks.sort_by_key(|stack| (stack.item_hash, stack.bucket_hash, stack.quantity));
            groups.push(CharacterPostmaster {
                character_id: character.id,
                class: character.class,
                instances,
                stacks,
            });
        }
        Ok(PostmasterResult {
            metadata,
            profile_currencies_coverage: snapshot.profile_currencies_coverage,
            characters: groups,
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
    ) -> Result<destiny_core::SubclassResult, AppError>
    where
        R: destiny_core::ports::WorldDefinitionRepository,
    {
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
        let mut clarity_cache = crate::clarity::ClarityCache::default();
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
                        Some((|| {
                            let description =
                                self.repository().lookup_item_description(plug.hash)?;
                            let (clarity, clarity_match) = clarity_cache.resolve(
                                self.repository(),
                                &plug,
                                Some(instance.item_hash),
                            )?;
                            Ok(destiny_core::SubclassSlot {
                                kind,
                                plug_hash: plug.hash,
                                empty: plug.name.starts_with("Empty ")
                                    && plug.name.ends_with(" Socket"),
                                description,
                                clarity,
                                clarity_match,
                                name: Some(plug.name),
                            })
                        })())
                    })
                    .collect::<Result<Vec<_>, AppError>>()?;
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

    /// Lists selectable subclass plugs; raw flags are retained even when status is unknown.
    /// Reference selection uses all characters, before applying the selector.
    /// Equal timestamps choose the lexicographically smallest ID for determinism.
    pub fn subclass_options(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        element: Option<destiny_core::SubclassElement>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassOptionsResult, AppError>
    where
        R: destiny_core::ports::WorldDefinitionRepository,
    {
        use destiny_core::{
            CharacterSubclassOptions, ElementOptions, OptionGroup, OptionStatus, PlugSetScope,
            SubclassElement as E, SubclassOption, SubclassPlugKind as K,
        };
        self.require_definitions()?;
        self.synchronize(policy, now)?;
        let snapshot = self.committed_snapshot()?;
        let reference = snapshot
            .characters
            .iter()
            .filter(|c| c.date_last_played.is_some())
            .max_by(|a, b| {
                a.date_last_played
                    .cmp(&b.date_last_played)
                    .then_with(|| b.id.as_str().cmp(a.id.as_str()))
            });
        let reference_character_id = reference.map(|c| c.id.clone());
        let reference_class = reference.and_then(|c| c.class.as_deref());
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
            None => snapshot.characters.clone(),
        };
        let mut enrichment = InstanceEnrichment::default();
        let mut clarity_cache = crate::clarity::ClarityCache::default();
        let mut output = Vec::new();
        for character in characters {
            let class = character.class.as_deref();
            let status_reliable = class
                .zip(reference_class)
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
            let mut deduped = std::collections::BTreeMap::<ItemHash, (bool, bool)>::new();
            for entry in &snapshot.plug_set_entries {
                if matches!(&entry.scope, PlugSetScope::Character(id) if id != &character.id) {
                    continue;
                }
                let flags = deduped.entry(entry.plug_hash).or_default();
                flags.0 |= entry.can_insert;
                flags.1 |= entry.enabled;
            }
            let mut grouped = Vec::new();
            for (hash, (can_insert, enabled)) in deduped {
                for definition in
                    enrichment.resolve_plugs(self.repository(), std::iter::once(hash))?
                {
                    let Some((prefix, plug_element, kind)) = definition
                        .plug_category
                        .as_deref()
                        .and_then(destiny_core::parse_subclass_category)
                    else {
                        continue;
                    };
                    if (prefix != "shared"
                        && !class.is_some_and(|c| c.eq_ignore_ascii_case(prefix)))
                        || element.is_some_and(|e| plug_element != Some(e))
                    {
                        continue;
                    }
                    let status = if !status_reliable {
                        OptionStatus::Unknown
                    } else if can_insert {
                        OptionStatus::Unlocked
                    } else {
                        OptionStatus::Locked
                    };
                    let (clarity, clarity_match) =
                        clarity_cache.resolve(self.repository(), &definition, None)?;
                    grouped.push((
                        plug_element,
                        kind,
                        SubclassOption {
                            clarity,
                            clarity_match,
                            plug_hash: hash,
                            name: Some(definition.name),
                            description: self.repository().lookup_item_description(hash)?,
                            status,
                            can_insert,
                            enabled,
                        },
                    ));
                }
            }
            let mut elements = Vec::new();
            for element in [
                Some(E::Prismatic),
                Some(E::Arc),
                Some(E::Solar),
                Some(E::Void),
                Some(E::Stasis),
                Some(E::Strand),
                None,
            ] {
                let mut groups = Vec::new();
                for kind in [
                    K::Super,
                    K::ClassAbility,
                    K::Movement,
                    K::Melee,
                    K::Grenade,
                    K::Aspect,
                    K::Fragment,
                    K::Transcendence,
                ] {
                    let mut options: Vec<_> = grouped
                        .iter()
                        .filter(|(e, k, _)| *e == element && *k == kind)
                        .map(|(_, _, option)| option.clone())
                        .collect();
                    options.sort_by(|a, b| (&a.name, a.plug_hash).cmp(&(&b.name, b.plug_hash)));
                    if !options.is_empty() {
                        groups.push(OptionGroup { kind, options });
                    }
                }
                if !groups.is_empty() {
                    elements.push(ElementOptions { element, groups });
                }
            }
            output.push(CharacterSubclassOptions {
                character_id: character.id,
                class: character.class,
                status_reliable,
                elements,
            });
        }
        Ok(destiny_core::SubclassOptionsResult {
            reference_character_id,
            characters: output,
        })
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
        let mut execution = request
            .query
            .as_ref()
            .map(destiny_core::search_query::ResolvedQuery::new);
        if let Some(typed) = &request.query {
            let execution = execution
                .as_mut()
                .expect("typed query has execution context");
            for name in destiny_core::search_query::referenced_stats(typed) {
                if self.repository().stat_definition_count()? == 0 {
                    return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
                }
                // Exact normalized catalog lookup, never fuzzy name expansion.
                let hashes = self.repository().stat_hashes_by_name(&name)?;
                if hashes.is_empty() {
                    return Err(AppError::new(ErrorCategory::UnknownStatName));
                }
                let mut displays = Vec::new();
                for hash in hashes {
                    let stat = self
                        .repository()
                        .lookup_stat(hash)?
                        .ok_or_else(invalid_repository_state)?;
                    displays.push(stat.name().to_owned());
                    execution.stats.push(stat);
                }
                let names = resolved.stats.entry(name).or_default();
                names.extend(displays);
                names.sort();
                names.dedup();
            }
        }
        let outcome = self.synchronize(policy, now)?;
        let metadata = query_metadata(&outcome, policy, now);
        if let Some(typed) = &request.query {
            let selectors = destiny_core::search_query::referenced_characters(typed);
            if !selectors.is_empty() {
                let snapshot = self.committed_snapshot()?;
                for selector in selectors {
                    let character = crate::weekly_selection::select_weekly_character(
                        &snapshot.characters,
                        Some(&destiny_core::CharacterId::new(&selector)),
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
                    execution
                        .as_mut()
                        .expect("typed query has execution context")
                        .characters
                        .insert(selector, character.id);
                }
            }
        }

        let candidates = self
            .sync
            .repository()
            .search_definitions(&request.definitions)?;

        let instance_query = if request.definitions.has_filters() || request.query.is_some() {
            let mut query = request.instances.clone().unwrap_or(InstanceSearchQuery {
                offset: 0,
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
            if request.definitions.has_filters() {
                query.definitions = Some(Box::new(request.definitions.clone()));
            }
            Some(query)
        } else {
            request.instances.clone()
        };
        let mut hits = Vec::new();
        let mut next_offset = None;
        if let Some(instance_query) = &instance_query {
            let (instances, more) = self
                .sync
                .repository()
                .search_instance_query_page(instance_query, execution.as_ref())?;
            if more {
                next_offset = instance_query
                    .offset
                    .checked_add(instances.len())
                    .filter(|offset| *offset <= destiny_core::MAX_SEARCH_OFFSET);
            }
            hits.reserve(instances.len());
            let mut enrichment = InstanceEnrichment::default();
            for instance in instances {
                let definition = self.resolve_definition(instance.item_hash)?;
                hits.push(enrichment.resolve(self.repository(), definition, instance)?);
            }
        }

        Ok(InventorySearchResult {
            next_offset,
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
    ) -> Result<Resolution<ItemInspection>, AppError>
    where
        R: destiny_core::ports::WorldDefinitionRepository,
    {
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
        let mut enrichment = InstanceEnrichment::default();
        let hit = enrichment.resolve(self.repository(), definition, instance)?;
        let plug_details = enrichment.inspect_details(self.repository(), &hit)?;
        Ok(Resolution::Unique(ItemInspection {
            plug_details,
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

/// Build the shared text search used for exact owned-name resolution.
pub fn owned_name_request(name: &str) -> Result<SearchInventoryRequest, AppError> {
    let definitions = DefinitionSearchQuery {
        text: Some(destiny_core::SearchTerm::new(name)?),
        item_type: None,
        tier: None,
        tags: Vec::new(),
        element: None,
        slot: None,
        class_type: None,
        limit: destiny_core::MAX_SEARCH_LIMIT,
    };
    let instances = InstanceSearchQuery {
        offset: 0,
        sort: None,
        include_available: false,
        item_hashes: destiny_core::ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        character_id: None,
        location: None,
        equipped: None,
        definitions: Some(Box::new(definitions.clone())),
        min_power: None,
        stats: Vec::new(),
        limit: destiny_core::MAX_SEARCH_LIMIT,
    };
    Ok(SearchInventoryRequest::new(definitions, Some(instances)))
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
    /// Resolve only normalized exact owned names, across every search page.
    fn owned_name_hits(
        &mut self,
        name: &str,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<InventorySearchResult, AppError> {
        let mut request = owned_name_request(name)?;
        let normalized = destiny_core::query::normalize_search_text(name);
        let mut result = self.search_inventory(&request, policy, now)?;
        let mut hits = std::mem::take(&mut result.hits);
        while let Some(offset) = result.next_offset {
            request.instances.as_mut().expect("owned name query").offset = offset;
            let page = self.search_inventory(&request, policy, now)?;
            hits.extend(page.hits);
            result.next_offset = page.next_offset;
        }
        hits.retain(|hit| {
            destiny_core::query::normalize_search_text(&hit.definition.name) == normalized
        });
        result.hits = hits;
        result.candidates.clear();
        Ok(result)
    }

    /// Reads committed Postmaster holdings for one selector or all owned characters.
    fn postmaster(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::PostmasterResult, AppError>;

    fn subclass_options(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        element: Option<destiny_core::SubclassElement>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassOptionsResult, AppError>;

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
        + destiny_core::StatDefinitionRepository
        + destiny_core::ports::WorldDefinitionRepository,
{
    fn postmaster(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::PostmasterResult, AppError> {
        QueryService::postmaster(self, character_id, policy, now)
    }

    fn subclass_options(
        &mut self,
        character_id: Option<&destiny_core::CharacterId>,
        element: Option<destiny_core::SubclassElement>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::SubclassOptionsResult, AppError> {
        QueryService::subclass_options(self, character_id, element, policy, now)
    }

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
        ProfileSyncOutcome::SkippedFresh { .. } | ProfileSyncOutcome::DiscardedNotNewer { .. } => {
            now
        }
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
        clarity_entries: Vec<destiny_core::ClarityInsight>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            let version = DefinitionVersion::new(DEFINITION_VERSION);
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions.clone()),
                definitions: InMemoryDefinitionRepository::new(version, definitions),
                clarity_entries: Vec::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn describe(&mut self, definitions: Vec<ItemDefinition>, hash: i64, text: &str) {
            let input = destiny_core::DefinitionReplacementInput {
                definitions: destiny_core::ManifestDefinitions {
                    version: DefinitionVersion::new(DEFINITION_VERSION),
                    definitions,
                },
                checked_at: NOW,
                source: SourceMarker::Manifest,
            };
            let description = destiny_core::ItemDescription::new(ItemHash::new(hash), text.into());
            self.definitions
                .replace_definitions_with_descriptions(input, vec![description.unwrap()])
                .unwrap();
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

        fn search_instance_query_page(
            &self,
            query: &InstanceSearchQuery,
            execution: Option<&destiny_core::search_query::ResolvedQuery>,
        ) -> Result<(Vec<ItemInstance>, bool), AppError> {
            if execution.is_none() {
                return self.search_instance_page(query);
            }
            self.record("search_instances");
            self.search.search_instance_query_page(query, execution)
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

    impl destiny_core::ports::WorldDefinitionRepository for CompositeRepository {
        fn lookup_clarity_by_name(
            &self,
            name: &str,
        ) -> Result<Vec<destiny_core::ClarityInsight>, AppError> {
            self.record("lookup_clarity_by_name");
            Ok(self
                .clarity_entries
                .iter()
                .filter(|entry| entry.name == name)
                .cloned()
                .collect())
        }
        fn lookup_clarity_by_hash(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ClarityInsight>, AppError> {
            self.record("lookup_clarity_by_hash");
            if let Some(entry) = self.clarity_entries.iter().find(|entry| entry.hash == hash) {
                return Ok(Some(entry.clone()));
            }
            Ok(self
                .definitions
                .lookup_item_description(ItemHash::new(i64::from(hash)))?
                .map(|_| destiny_core::ClarityInsight {
                    hash,
                    name: "Community plug".into(),
                    kind: "Weapon Trait".into(),
                    item_hash: None,
                    item_name: None,
                    text: "  Community research.\nSecond line.\n".into(),
                }))
        }
        fn lookup_item_description(&self, hash: ItemHash) -> Result<Option<String>, AppError> {
            self.definitions.lookup_item_description(hash)
        }
        fn lookup_milestone(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::MilestoneDefinition>, AppError> {
            self.definitions.lookup_milestone(hash)
        }
        fn lookup_activity(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityDefinition>, AppError> {
            self.definitions.lookup_activity(hash)
        }
        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityModifierDefinition>, AppError> {
            self.definitions.lookup_activity_modifier(hash)
        }
        fn lookup_objective(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ObjectiveDefinition>, AppError> {
            self.definitions.lookup_objective(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityTypeDefinition>, AppError> {
            self.definitions.lookup_activity_type(hash)
        }
        fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
            self.definitions.lookup_loadout_name(hash)
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            self.definitions.world_definition_counts()
        }
        fn publish_manifest(
            &mut self,
            items: destiny_core::DefinitionReplacementInput,
            world: destiny_core::WorldDefinitionsAcquisition,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.publish_manifest(items, world)
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

    #[test]
    fn clarity_cache_rejects_cross_category_name_fallback() {
        let repository = CompositeRepository {
            clarity_entries: vec![destiny_core::ClarityInsight {
                hash: 1,
                name: "Ascension".into(),
                kind: "Weapon Trait".into(),
                item_hash: None,
                item_name: None,
                text: "Unrelated".into(),
            }],
            ..CompositeRepository::default()
        };
        let mut plug = definition(
            2835214901,
            "Ascension",
            Some("Arc Aspect | Light Ability"),
            vec![],
        );
        plug.plug_category = Some("hunter.prism.aspects".into());
        assert_eq!(
            crate::clarity::ClarityCache::default()
                .resolve(&repository, &plug, None)
                .unwrap(),
            (None, None)
        );
    }

    #[test]
    fn clarity_cache_unrelated_kind_cannot_create_ambiguity() {
        let repository = CompositeRepository {
            clarity_entries: [("Subclass Aspect", "Aspect"), ("Weapon Trait", "Unrelated")]
                .into_iter()
                .enumerate()
                .map(|(i, (kind, text))| destiny_core::ClarityInsight {
                    hash: i as u32 + 1,
                    name: "Ascension".into(),
                    kind: kind.into(),
                    item_hash: None,
                    item_name: None,
                    text: text.into(),
                })
                .collect(),
            ..CompositeRepository::default()
        };
        let mut plug = definition(
            2835214901,
            "Ascension",
            Some("Arc Aspect | Light Ability"),
            vec![],
        );
        plug.plug_category = Some("hunter.prism.aspects".into());
        assert_eq!(
            crate::clarity::ClarityCache::default()
                .resolve(&repository, &plug, None)
                .unwrap(),
            (Some("Aspect".into()), None)
        );
    }

    #[test]
    fn clarity_cache_preserves_semantic_variants_and_fails_closed() {
        let repository = CompositeRepository {
            clarity_entries: [
                ("Weapon Trait", "Base"),
                ("Weapon Trait Enhanced", "Enhanced"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (kind, text))| destiny_core::ClarityInsight {
                hash: i as u32 + 1,
                name: "Plug".into(),
                kind: kind.into(),
                item_hash: None,
                item_name: None,
                text: text.into(),
            })
            .collect(),
            ..CompositeRepository::default()
        };
        let mut cache = crate::clarity::ClarityCache::default();
        let mut plug = definition(99, "Plug", Some("Origin Trait"), vec![]);
        plug.plug_category = Some("origins".into());
        assert_eq!(
            cache.resolve(&repository, &plug, None).unwrap(),
            (Some("Base".into()), None)
        );
        plug.item_type = Some("Enhanced Origin Trait".into());
        assert_eq!(
            cache.resolve(&repository, &plug, None).unwrap(),
            (Some("Enhanced".into()), None)
        );
        for category in [None, Some("unknown"), Some("hunter.prism.aspects")] {
            plug.plug_category = category.map(str::to_owned);
            assert_eq!(
                cache.resolve(&repository, &plug, None).unwrap(),
                (None, None)
            );
        }
        plug.hash = ItemHash::new(1);
        assert_eq!(
            cache.resolve(&repository, &plug, None).unwrap(),
            (Some("Base".into()), None)
        );
    }

    #[test]
    fn clarity_cache_bounds_reads_without_caching_parent_specific_results() {
        let repository = CompositeRepository {
            clarity_entries: [("A", 10), ("B", 20)]
                .into_iter()
                .enumerate()
                .map(|(i, (text, parent))| destiny_core::ClarityInsight {
                    hash: i as u32 + 1,
                    name: "Plug".into(),
                    kind: "Weapon Trait".into(),
                    item_hash: Some(parent),
                    item_name: None,
                    text: text.into(),
                })
                .collect(),
            ..CompositeRepository::default()
        };
        let mut cache = crate::clarity::ClarityCache::default();
        let mut plug = definition(99, "Plug", Some("Enhanced Origin Trait"), vec![]);
        plug.plug_category = Some("origins".into());
        for _ in 0..3 {
            for (parent, expected) in [(10, Some("A")), (20, Some("B")), (30, None)] {
                let (text, marker) = cache
                    .resolve(&repository, &plug, Some(ItemHash::new(parent)))
                    .unwrap();
                assert_eq!(text.as_deref(), expected);
                assert_eq!(marker.as_deref(), expected.map(|_| "variant"));
            }
        }
        plug.hash = ItemHash::new(100);
        cache.resolve(&repository, &plug, None).unwrap();
        assert_eq!(
            repository.recorded_calls(),
            [
                "lookup_clarity_by_hash",
                "lookup_clarity_by_name",
                "lookup_clarity_by_hash"
            ]
        );
        // Exact hits never query names, and misses are cached too.
        plug.hash = ItemHash::new(1);
        for _ in 0..3 {
            cache.resolve(&repository, &plug, None).unwrap();
        }
        plug.hash = ItemHash::new(200);
        plug.name = "Missing".into();
        for _ in 0..3 {
            assert_eq!(
                cache.resolve(&repository, &plug, None).unwrap(),
                (None, None)
            );
        }
        assert_eq!(
            repository
                .recorded_calls()
                .iter()
                .filter(|&&call| call == "lookup_clarity_by_hash")
                .count(),
            4
        );
        assert_eq!(
            repository
                .recorded_calls()
                .iter()
                .filter(|&&call| call == "lookup_clarity_by_name")
                .count(),
            2
        );
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
            date_last_played: None,
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
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
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
            profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
            stacks: Vec::new(),
            plug_set_entries: Vec::new(),
            membership_id: membership.membership_id().clone(),
            membership_type: membership.membership_type(),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at,
            source: SourceMarker::Profile,
            characters,
            items,
        }
    }

    struct ArtifactFake {
        profile: SharedGateway,
        artifact: destiny_core::ArtifactSnapshot,
    }
    impl ProfileGateway for ArtifactFake {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.profile.resolve_membership()
        }
        fn load_inventory(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.profile.load_inventory(membership)
        }
    }
    impl destiny_core::ArtifactGateway for ArtifactFake {
        fn load_artifact(
            &mut self,
            requested: &DestinyMembership,
        ) -> Result<destiny_core::ArtifactSnapshot, AppError> {
            assert_eq!(requested, &membership());
            Ok(self.artifact.clone())
        }
    }
    fn artifact_service(
        profile_present: bool,
        stale: bool,
    ) -> crate::ArtifactService<ArtifactFake, CompositeRepository> {
        artifact_service_with(profile_present, stale, |_, _| {})
    }
    fn artifact_service_with(
        profile_present: bool,
        stale: bool,
        configure: impl FnOnce(&mut ArtifactFake, &mut CompositeRepository),
    ) -> crate::ArtifactService<ArtifactFake, CompositeRepository> {
        use destiny_core::{ArtifactProfileState, ArtifactSnapshot};
        let shared = gateway();
        let mut hunter = character("1");
        hunter.class = Some("Hunter".into());
        let mut titan = character("2");
        titan.class = Some("Titan".into());
        let mut repository = CompositeRepository::with_definitions(vec![
            definition(10, "Artifact", None, vec![]),
            definition(20, "First", None, vec![]),
            definition(21, "Second", None, vec![]),
        ]);
        let input = snapshot_input(NOW, vec![hunter, titan], vec![]);
        if stale {
            queue_refresh(&shared, input);
        } else {
            repository.commit(input);
        }
        let artifact = ArtifactSnapshot {
            profile: profile_present.then_some(ArtifactProfileState {
                points: 12,
                points_cap: 12,
                progress_to_next_level: 34,
                next_level_at: 100,
            }),
        };
        let mut fake = ArtifactFake {
            profile: SharedGateway(shared),
            artifact,
        };
        configure(&mut fake, &mut repository);
        crate::ArtifactService::new(ProfileSyncService::new_with_definitions(fake, repository))
    }
    #[test]
    fn artifact_equipped_snapshot_survives_missing_live_points() {
        let mut service = artifact_service_with(false, false, |_, repository| {
            let mut artifact = definition(30, "Tablet of Ruin", Some("Artifact"), vec![]);
            artifact.bucket_hash = Some(destiny_core::ARTIFACT_BUCKET_HASH);
            let mut definitions = vec![artifact];
            for hash in 40..48 {
                let name = if hash == 47 {
                    "Empty Artifact Mod".into()
                } else {
                    format!("Perk {}", hash - 39)
                };
                let mut perk = definition(hash, &name, None, vec![]);
                perk.plug_category = Some("artifact_perks".into());
                definitions.push(perk);
            }
            let mut shader = definition(48, "Shader", None, vec![]);
            shader.plug_category = Some("shader".into());
            definitions.push(shader);
            repository.definitions =
                CompositeRepository::with_definitions(definitions.clone()).definitions;
            repository.describe(definitions, 40, "Official artifact tooltip.");
            let mut item = instance(
                "tablet",
                30,
                ItemLocation::Character(CharacterId::new("1")),
                vec![],
            );
            item.equipped = true;
            item.plugs = (40..=48)
                .map(|hash| destiny_core::PlugSummary {
                    plug_hash: ItemHash::new(hash),
                    enabled: true,
                })
                .collect();
            let mut unequipped = item.clone();
            unequipped.instance_id = ItemInstanceId::new("unequipped");
            unequipped.equipped = false;
            let mut vault = item.clone();
            vault.instance_id = ItemInstanceId::new("vault");
            vault.location = ItemLocation::Vault;
            repository.commit(snapshot_input(
                NOW,
                vec![character("1"), character("2")],
                vec![item, unequipped, vault],
            ));
        });
        let result = service.artifact(None, TTL, NOW).unwrap();
        assert!(result.points.is_none());
        let equipped = result.characters[0]
            .equipped
            .as_ref()
            .expect("snapshot artifact");
        assert_eq!(equipped.instance_id, ItemInstanceId::new("tablet"));
        assert_eq!(equipped.item_hash, ItemHash::new(30));
        assert_eq!(equipped.name.as_deref(), Some("Tablet of Ruin"));
        assert_eq!(
            equipped
                .slots
                .iter()
                .map(|slot| slot.plug_hash.value())
                .collect::<Vec<_>>(),
            (40..48).collect::<Vec<_>>()
        );
        assert_eq!(
            equipped.slots[0].description.as_deref(),
            Some("Official artifact tooltip.")
        );
        assert_eq!(equipped.slots[1].description, None);
        assert_eq!(
            equipped.slots[0].clarity.as_deref(),
            Some("  Community research.\nSecond line.\n")
        );
        assert_eq!(equipped.slots[0].clarity_match, None);
        assert_eq!(equipped.slots[1].clarity, None);
        assert!(equipped.slots[..7].iter().all(|slot| !slot.empty));
        assert!(equipped.slots[7].empty);
        assert_eq!(
            equipped.slots[7].name.as_deref(),
            Some("Empty Artifact Mod")
        );
        assert!(result.characters[1].equipped.is_none());
    }

    #[test]
    fn artifact_projects_points_and_synced_characters() {
        use crate::ArtifactOperations;
        let result =
            ArtifactOperations::artifact(&mut artifact_service(true, true), None, TTL, NOW)
                .unwrap();
        assert_eq!(
            result.points,
            Some(crate::ArtifactPoints {
                points: 12,
                points_cap: 12,
                progress_to_next_level: 34,
                next_level_at: 100
            })
        );
        assert_eq!(result.characters.len(), 2);
        assert!(result.characters.iter().all(|c| c.equipped.is_none()));
    }
    #[test]
    fn artifact_selects_id_or_case_insensitive_class_and_allows_missing_profile() {
        for selector in ["1", "hUnTeR"] {
            let result = artifact_service(false, false)
                .artifact(Some(&CharacterId::new(selector)), TTL, NOW)
                .unwrap();
            assert!(result.points.is_none());
            assert_eq!(result.characters.len(), 1);
            assert_eq!(result.characters[0].character_id, CharacterId::new("1"));
            assert!(result.characters[0].equipped.is_none());
        }
        let result = artifact_service(true, false)
            .artifact(Some(&CharacterId::new("Titan")), TTL, NOW)
            .unwrap();
        assert!(result.characters[0].equipped.is_none());
        assert_eq!(
            artifact_service(true, false)
                .artifact(Some(&CharacterId::new("missing")), TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
    }

    #[test]
    fn artifact_gates_definitions_and_propagates_sync_failure() {
        let mut service = artifact_service_with(true, true, |_, repository| {
            repository.definitions = InMemoryDefinitionRepository::default();
        });
        assert_eq!(
            service.artifact(None, TTL, NOW).unwrap_err().category(),
            ErrorCategory::DefinitionsUnavailable
        );
        let mut service = artifact_service_with(true, false, |fake, repository| {
            repository.inventory = InMemoryInventoryRepository::new();
            fake.profile
                .0
                .borrow_mut()
                .queue_membership(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        });
        assert_eq!(
            service.artifact(None, TTL, NOW).unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
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
            offset: 0,
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
        SearchInventoryRequest::new(definitions, instances)
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
    fn subclass_options_groups_filters_scopes_and_status() {
        use destiny_core::{
            OptionStatus, PlugSetEntry, PlugSetScope, SubclassElement, SubclassPlugKind,
        };
        let cases = [
            (1, "Zulu", "hunter.prism.supers"),
            (2, "Alpha", "hunter.prism.supers"),
            (3, "Dodge", "hunter.prism.class_abilities"),
            (4, "Jump", "hunter.prism.movement"),
            (5, "Melee", "hunter.prism.melee"),
            (6, "Grenade", "hunter.prism.prism_grenade"),
            (7, "Aspect", "hunter.prism.aspects"),
            (8, "Fragment", "hunter.prism.fragments"),
            (9, "Transcendence", "hunter.prism.transcendence"),
            (10, "Arc", "shared.arc.grenades"),
            (11, "Solar", "shared.solar.grenades"),
            (12, "Void", "shared.void.grenades"),
            (13, "Stasis", "shared.stasis.grenades"),
            (14, "Strand", "shared.strand.grenades"),
            (15, "Shared", "shared.fragments"),
            (16, "Titan", "titan.prism.supers"),
            (17, "Personal", "shared.fragments"),
        ];
        let definitions: Vec<_> = cases
            .iter()
            .map(|(hash, name, category)| {
                let mut plug = definition(*hash, name, None, vec![]);
                plug.plug_category = Some((*category).into());
                plug
            })
            .collect();
        let mut hunter = character("1");
        hunter.class = Some("Hunter".into());
        hunter.date_last_played = Some(Timestamp::new(200));
        let mut titan = character("2");
        titan.class = Some("Titan".into());
        titan.date_last_played = Some(Timestamp::new(100));
        let mut second_hunter = character("3");
        second_hunter.class = Some("Hunter".into());
        let mut input = snapshot_input(
            fresh_checked_at(),
            vec![hunter, titan, second_hunter],
            vec![],
        );
        input.plug_set_entries = (1..=16)
            .map(|hash| PlugSetEntry {
                scope: PlugSetScope::Profile,
                plug_hash: ItemHash::new(hash),
                can_insert: hash != 2,
                enabled: false,
            })
            .collect();
        input.plug_set_entries.push(PlugSetEntry {
            scope: PlugSetScope::Character(CharacterId::new("1")),
            plug_hash: ItemHash::new(2),
            can_insert: true,
            enabled: true,
        });
        input.plug_set_entries.push(PlugSetEntry {
            scope: PlugSetScope::Character(CharacterId::new("2")),
            plug_hash: ItemHash::new(17),
            can_insert: false,
            enabled: true,
        });
        let mut repository = CompositeRepository::with_definitions(definitions.clone());
        repository.describe(definitions, 2, "Official option tooltip.");
        repository.commit(input);
        let shared = gateway();
        let mut service = service(&shared, repository);
        let result = service.subclass_options(None, None, TTL, NOW).unwrap();
        assert_eq!(result.reference_character_id, Some(CharacterId::new("1")));
        let hunter = &result.characters[0];
        assert!(hunter.status_reliable);
        assert_eq!(
            hunter
                .elements
                .iter()
                .map(|e| e.element)
                .collect::<Vec<_>>(),
            vec![
                Some(SubclassElement::Prismatic),
                Some(SubclassElement::Arc),
                Some(SubclassElement::Solar),
                Some(SubclassElement::Void),
                Some(SubclassElement::Stasis),
                Some(SubclassElement::Strand),
                None,
            ]
        );
        assert_eq!(
            hunter.elements[0]
                .groups
                .iter()
                .map(|g| g.kind)
                .collect::<Vec<_>>(),
            vec![
                SubclassPlugKind::Super,
                SubclassPlugKind::ClassAbility,
                SubclassPlugKind::Movement,
                SubclassPlugKind::Melee,
                SubclassPlugKind::Grenade,
                SubclassPlugKind::Aspect,
                SubclassPlugKind::Fragment,
                SubclassPlugKind::Transcendence,
            ]
        );
        let supers = &hunter.elements[0].groups[0].options;
        assert_eq!(supers.len(), 2);
        assert_eq!(supers[0].name.as_deref(), Some("Alpha"));
        assert_eq!(
            supers[0].description.as_deref(),
            Some("Official option tooltip.")
        );
        assert_eq!(supers[1].description, None);
        assert_eq!(
            supers[0].clarity.as_deref(),
            Some("  Community research.\nSecond line.\n")
        );
        assert_eq!(supers[0].clarity_match, None);
        assert_eq!(supers[1].clarity, None);
        assert!(supers[0].can_insert && supers[0].enabled);
        assert_eq!(supers[0].status, OptionStatus::Unlocked);
        assert_eq!(hunter.elements.last().unwrap().groups[0].options.len(), 1);
        let titan = &result.characters[1];
        assert!(!titan.status_reliable);
        assert!(
            titan
                .elements
                .iter()
                .flat_map(|e| &e.groups)
                .flat_map(|g| &g.options)
                .all(|o| o.status == OptionStatus::Unknown)
        );
        assert_eq!(titan.elements.last().unwrap().groups[0].options.len(), 2);
        let other_hunter = &result.characters[2];
        assert!(other_hunter.status_reliable);
        assert_eq!(
            other_hunter.elements[0].groups[0].options[0].status,
            OptionStatus::Locked
        );
        let filtered = service
            .subclass_options(
                Some(&CharacterId::new("2")),
                Some(SubclassElement::Arc),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(
            filtered.reference_character_id,
            result.reference_character_id
        );
        assert_eq!(filtered.characters.len(), 1);
        assert_eq!(
            filtered.characters[0].elements,
            vec![titan.elements[1].clone()]
        );
        assert!(
            service
                .subclass_options(Some(&CharacterId::new("missing")), None, TTL, NOW)
                .is_err()
        );
        assert_eq!(shared.borrow().membership_calls(), 0);
    }

    #[test]
    fn subclass_options_without_last_played_is_unreliable() {
        let shared = gateway();
        let mut service = fresh_service(&shared, vec![character("a")], vec![]);
        let result = service.subclass_options(None, None, TTL, NOW).unwrap();
        assert!(result.reference_character_id.is_none());
        assert!(!result.characters[0].status_reliable);
        assert!(result.characters[0].elements.is_empty());
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
        let mut repository = CompositeRepository::with_definitions(definitions.clone());
        repository.describe(definitions, 20, "Official super tooltip.");
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
        assert_eq!(
            config.slots[0].description.as_deref(),
            Some("Official super tooltip.")
        );
        assert_eq!(config.slots[1].description, None);
        assert_eq!(
            config.slots[0].clarity.as_deref(),
            Some("  Community research.\nSecond line.\n")
        );
        assert_eq!(config.slots[0].clarity_match, None);
        assert_eq!(config.slots[1].clarity, None);
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
        let mut repository = CompositeRepository::with_definitions(definitions.clone());
        repository.describe(definitions, 60, "Official rampage tooltip.\nSecond line.");
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
        assert_eq!(inspected.plug_details.len(), 3);
        assert_eq!(inspected.plug_details[0].hash, ItemHash::new(61));
        assert_eq!(inspected.plug_details[0].description, None);
        assert_eq!(inspected.plug_details[0].clarity, None);
        assert_eq!(
            inspected.plug_details[1].clarity.as_deref(),
            Some("  Community research.\nSecond line.\n")
        );
        assert_eq!(inspected.plug_details[1].clarity_match, None);
        assert_eq!(
            inspected.plug_details[1].description.as_deref(),
            Some("Official rampage tooltip.\nSecond line.")
        );
        assert_eq!(
            inspected.plug_details[2].role,
            destiny_core::InspectionPlugRole::AvailablePerk
        );
    }

    #[test]
    fn inspect_armor_intrinsic_with_description() {
        assert_inspection_intrinsic(destiny_core::domain::ItemSlot::Legs);
    }

    #[test]
    fn inspect_weapon_intrinsic_replaces_perk_detail_without_changing_names() {
        assert_inspection_intrinsic(destiny_core::domain::ItemSlot::Kinetic);
    }

    fn assert_inspection_intrinsic(slot: destiny_core::domain::ItemSlot) {
        let shared = gateway();
        let mut definitions = standard_definitions();
        definitions
            .iter_mut()
            .find(|definition| definition.hash == ItemHash::new(1))
            .unwrap()
            .slot = Some(slot);
        for (hash, name, category) in [
            (60, "Intrinsic", "intrinsics"),
            (61, "Barrel", "barrels"),
            (62, "Not intrinsic", "intrinsics.extra"),
            (63, "Alternative intrinsic", "intrinsics"),
            (64, "Second intrinsic", "intrinsics"),
        ] {
            let mut plug = definition(hash, name, None, vec![]);
            plug.plug_category = Some(category.into());
            definitions.push(plug);
        }
        let mut repository = CompositeRepository::with_definitions(definitions.clone());
        repository.describe(definitions, 60, "Official intrinsic tooltip.\nSecond line.");
        let mut item = instance("owned", 1, ItemLocation::Vault, vec![]);
        // Intrinsics need not be the first socket; unknown and repeated plugs are safe.
        item.plugs = [61, 99, 60, 62, 64, 60]
            .into_iter()
            .map(|hash| destiny_core::PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        item.available_plugs = vec![destiny_core::AvailablePlug {
            socket_index: 8,
            plug_hash: ItemHash::new(63),
        }];
        repository.commit(snapshot_input(fresh_checked_at(), vec![], vec![item]));
        let mut service = service(&shared, repository);
        let result = service
            .search_inventory(
                &search_request(
                    definition_query(None, None, vec![], 1),
                    Some(instance_query(ItemHashFilter::Any, None, None, 1)),
                ),
                TTL,
                NOW,
            )
            .unwrap();
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
        let hit = &result.hits[0];
        assert_eq!(inspected.perk_names, hit.perk_names);
        assert_eq!(inspected.mod_names, hit.mod_names);
        assert_eq!(inspected.archetype, hit.archetype);
        assert_eq!(inspected.available_perks, hit.available_perks);
        assert_eq!(inspected.available_mods, hit.available_mods);
        if slot == destiny_core::domain::ItemSlot::Kinetic {
            assert_eq!(
                inspected.perk_names,
                ["Barrel", "Intrinsic", "Second intrinsic", "Intrinsic"]
            );
            assert_eq!(inspected.available_perks, ["Alternative intrinsic"]);
        } else {
            assert!(inspected.perk_names.is_empty());
            assert!(inspected.available_perks.is_empty());
        }
        let intrinsic = &inspected.plug_details[0];
        assert_eq!(intrinsic.hash, ItemHash::new(60));
        assert_eq!(intrinsic.role.as_str(), "intrinsic");
        assert_eq!(
            intrinsic.description.as_deref(),
            Some("Official intrinsic tooltip.\nSecond line.")
        );
        assert_eq!(inspected.plug_details[1].hash, ItemHash::new(64));
        assert_eq!(inspected.plug_details[1].role.as_str(), "intrinsic");
        assert_eq!(inspected.plug_details[1].description, None);
        assert_eq!(
            inspected
                .plug_details
                .iter()
                .filter(|plug| plug.hash == ItemHash::new(60))
                .count(),
            1
        );
        assert!(
            inspected
                .plug_details
                .iter()
                .all(|plug| plug.hash != ItemHash::new(62))
        );
        assert!(
            !inspected
                .plug_details
                .iter()
                .any(|plug| plug.hash == ItemHash::new(63) && plug.role.as_str() == "intrinsic")
        );
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
        let mut alternative = definition(64, "Heavy Ammo Finder", None, vec![]);
        alternative.plug_category = Some("enhancements.v2_head".into());
        let mut empty = definition(65, "Empty Mod Socket", None, vec![]);
        empty.plug_category = Some("enhancements.v2_head".into());
        definitions.extend([armor_mod, intrinsic, archetype, second, alternative, empty]);
        let mut repository = CompositeRepository::with_definitions(definitions.clone());
        repository.describe(definitions, 60, "Official mod tooltip.");
        let mut item = instance("owned", 1, ItemLocation::Vault, vec![]);
        item.available_plugs = [60, 64, 64, 65, 99]
            .into_iter()
            .enumerate()
            .map(|(index, hash)| destiny_core::AvailablePlug {
                socket_index: index as u8,
                plug_hash: ItemHash::new(hash),
            })
            .collect();
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
        assert_eq!(
            inspected
                .plug_details
                .iter()
                .map(|plug| plug.hash.value())
                .collect::<Vec<_>>(),
            [61, 60, 62, 64]
        );
        assert_eq!(
            inspected.plug_details[0].role,
            destiny_core::InspectionPlugRole::Intrinsic
        );
        assert_eq!(
            inspected.plug_details[2].role,
            destiny_core::InspectionPlugRole::Archetype
        );
        assert_eq!(inspected.available_mods, ["Heavy Ammo Finder"]);
        assert_eq!(
            inspected.plug_details[1].description.as_deref(),
            Some("Official mod tooltip.")
        );
        assert_eq!(
            inspected.plug_details[3].role,
            destiny_core::InspectionPlugRole::AvailableMod
        );
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
        let discarded = destiny_core::ProfileSyncOutcome::DiscardedNotNewer {
            metadata: old.clone(),
        };
        assert_eq!(
            super::query_metadata(&discarded, TTL, NOW).freshness,
            FreshnessStatus::Stale
        );
        let future_discarded = destiny_core::ProfileSyncOutcome::DiscardedNotNewer {
            metadata: metadata.clone(),
        };
        assert_eq!(
            super::query_metadata(&future_discarded, TTL, NOW).freshness,
            FreshnessStatus::Stale
        );
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
