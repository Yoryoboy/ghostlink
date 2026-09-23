//! Injectable read-service seam and translation onto the shared query service.
//!
//! [`InventoryReadService`] is the narrow, object-safe seam the future stdio
//! runtime calls: one method per semantic read, each taking an already-validated
//! request and returning a bounded MCP output or a sanitized [`McpError`].
//! [`QueryReadService`] is the production implementation. It composes the shared
//! application [`QueryService`] with an injected clock and freshness policy, so
//! the MCP layer adds no freshness, search, storage, or provider policy of its
//! own.
//!
//! # Argument bounds
//!
//! Converting the raw MCP arguments into a shared read request is where the
//! adapter bounds live. Definition and instance limits, tag counts, hash-candidate
//! counts, and search text lengths are all checked against the shared read
//! contract before the service is invoked, and the shared query types validate
//! again once constructed. A rejected argument produces the sanitized
//! [`McpError`] for [`ErrorCategory::ProviderValidation`] and never echoes the
//! offending value.
//!
//! # Identity
//!
//! [`DefinitionView`] carries a definition hash and [`InstanceView`] carries a
//! concrete instance identifier; the two are never interchanged. Search reports
//! definition candidates separately from instance hits, and inspection reports
//! the exact definition beside the concrete instance.

use destiny_app::{
    Character, CharacterId, CharacterListResult, Clock, Dataset, DefinitionRepository,
    DefinitionSearchQuery, ErrorCategory, FreshnessPolicy, FreshnessStatus, InspectItemRequest,
    InstanceSearchQuery, InventoryRepository, InventorySearchResult, ItemDefinition, ItemHash,
    ItemHashFilter, ItemInspection, ItemInstance, ItemInstanceId, ItemLocation, MAX_HASH_FILTERS,
    MAX_SEARCH_LIMIT, MAX_TAG_FILTERS, PerkHash, ProfileGateway, QueryMetadata, QueryService,
    Resolution, SearchInventoryRequest, SearchRepository, SearchTerm, SourceMarker,
};

use crate::schema::{
    CharacterView, DefinitionView, FoundInspection, InspectItemArgs, InspectItemOutput,
    InstanceQueryArgs, InstanceView, InventorySearchHitView, ListCharactersArgs,
    ListCharactersOutput, LocationView, MAX_IDENTIFIER_LENGTH, McpError, PerkView,
    QueryMetadataView, SearchInventoryArgs, SearchInventoryOutput,
};

/// A validated `list_characters` request. The tool takes no arguments.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ListCharactersRequest;

/// The narrow read seam the MCP runtime calls.
///
/// The trait is deliberately object-safe and free of SDK types, so a server can
/// hold `Box<dyn InventoryReadService>` and tests can inject any implementation
/// without touching `rmcp`. Requests are already validated and bounded; outputs
/// are the bounded MCP projections defined in [`crate::schema`].
pub trait InventoryReadService {
    /// Lists the committed characters.
    fn list_characters(
        &mut self,
        request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError>;

    /// Runs one bounded definition search and optional instance search.
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError>;

    /// Inspects exactly one concrete instance.
    fn inspect_item(&mut self, request: &InspectItemRequest)
    -> Result<InspectItemOutput, McpError>;
}

/// Composes the shared query service with an injected clock and freshness policy.
///
/// The clock supplies `now` and the policy supplies the freshness TTL, so the
/// MCP layer never reads an ambient clock and never chooses a TTL itself. Both
/// are injected by the caller that owns production composition.
pub struct QueryReadService<G, R, C> {
    queries: QueryService<G, R>,
    clock: C,
    policy: FreshnessPolicy,
}

impl<G, R, C> QueryReadService<G, R, C> {
    /// Wraps one composed query service with its clock and freshness policy.
    pub fn new(queries: QueryService<G, R>, clock: C, policy: FreshnessPolicy) -> Self {
        Self {
            queries,
            clock,
            policy,
        }
    }

    /// Borrows the composed query service.
    pub fn queries(&self) -> &QueryService<G, R> {
        &self.queries
    }

    /// Mutably borrows the composed query service for test or composition setup.
    pub fn queries_mut(&mut self) -> &mut QueryService<G, R> {
        &mut self.queries
    }
}

impl<G, R, C> InventoryReadService for QueryReadService<G, R, C>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    C: Clock,
{
    fn list_characters(
        &mut self,
        _request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .list_characters(self.policy, now)
            .map_err(McpError::from)?;
        Ok(list_characters_output(&result))
    }

    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .search_inventory(request, self.policy, now)
            .map_err(McpError::from)?;
        Ok(search_inventory_output(&result))
    }

    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<InspectItemOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let resolution = self
            .queries
            .inspect_item(request, self.policy, now)
            .map_err(McpError::from)?;
        inspect_item_output(&resolution)
    }
}

impl ListCharactersArgs {
    /// Validates and converts raw arguments into a `list_characters` request.
    pub fn into_request(self) -> ListCharactersRequest {
        ListCharactersRequest
    }
}

impl SearchInventoryArgs {
    /// Validates and converts raw arguments into a shared search request.
    ///
    /// All adapter bounds are enforced here, before any service call. The shared
    /// query types validate the constructed request again, so the two layers can
    /// never disagree on what is acceptable.
    pub fn into_request(self) -> Result<SearchInventoryRequest, McpError> {
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(invalid_arguments());
        }
        if self.tags.len() > MAX_TAG_FILTERS {
            return Err(invalid_arguments());
        }

        let definitions = DefinitionSearchQuery {
            text: optional_term(self.text.as_deref())?,
            item_type: optional_term(self.item_type.as_deref())?,
            tags: self
                .tags
                .iter()
                .map(|value| SearchTerm::new(value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(McpError::from)?,
            limit: self.limit,
        };
        definitions.validate().map_err(McpError::from)?;

        let instances = self
            .instances
            .map(InstanceQueryArgs::into_query)
            .transpose()?;

        Ok(SearchInventoryRequest {
            definitions,
            instances,
        })
    }
}

impl InstanceQueryArgs {
    /// Validates and converts one instance query against the shared bounds.
    fn into_query(self) -> Result<InstanceSearchQuery, McpError> {
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(invalid_arguments());
        }
        if let Some(character_id) = &self.character_id {
            validate_identifier(character_id)?;
        }

        let item_hashes = match self.item_hashes {
            None => ItemHashFilter::Any,
            Some(hashes) => {
                if hashes.len() > MAX_HASH_FILTERS {
                    return Err(invalid_arguments());
                }
                ItemHashFilter::AnyOf(hashes.into_iter().map(ItemHash::new).collect())
            }
        };

        let query = InstanceSearchQuery {
            item_hashes,
            perk_hash: self.perk_hash.map(PerkHash::new),
            character_id: self.character_id.map(CharacterId::new),
            limit: self.limit,
        };
        query.validate().map_err(McpError::from)?;
        Ok(query)
    }
}

impl InspectItemArgs {
    /// Validates and converts raw arguments into a shared inspection request.
    ///
    /// The identifier stays an [`ItemInstanceId`]; a display name never reaches
    /// this request. An empty or over-long identifier is rejected with the
    /// sanitized [`ErrorCategory::ProviderValidation`] category before the
    /// service is invoked.
    pub fn into_request(self) -> Result<InspectItemRequest, McpError> {
        validate_identifier(&self.instance_id)?;
        Ok(InspectItemRequest {
            instance_id: ItemInstanceId::new(self.instance_id),
        })
    }
}

/// Projects a shared character listing into its bounded MCP output.
pub fn list_characters_output(result: &CharacterListResult) -> ListCharactersOutput {
    ListCharactersOutput {
        characters: result.characters.iter().map(character_view).collect(),
        metadata: metadata_view(&result.metadata),
    }
}

/// Projects a shared inventory search into its bounded MCP output.
pub fn search_inventory_output(result: &InventorySearchResult) -> SearchInventoryOutput {
    SearchInventoryOutput {
        candidates: result.candidates.iter().map(definition_view).collect(),
        hits: result
            .hits
            .iter()
            .map(|hit| InventorySearchHitView {
                definition: definition_view(&hit.definition),
                instance: instance_view(&hit.instance),
            })
            .collect(),
        metadata: metadata_view(&result.metadata),
    }
}

/// Projects a shared inspection resolution into its bounded MCP output.
///
/// [`Resolution::Missing`] is an explicit successful outcome. The shared service
/// already fails closed on an ambiguous or inconsistent projection before this
/// projection runs; a direct caller of the mapping receives the same sanitized
/// category instead of a partial read.
pub fn inspect_item_output(
    resolution: &Resolution<ItemInspection>,
) -> Result<InspectItemOutput, McpError> {
    match resolution {
        Resolution::Missing => Ok(InspectItemOutput::Missing),
        Resolution::Unique(inspection) => Ok(InspectItemOutput::Found(Box::new(FoundInspection {
            definition: definition_view(&inspection.definition),
            instance: instance_view(&inspection.instance),
            metadata: metadata_view(&inspection.metadata),
        }))),
        Resolution::Ambiguous(_) => Err(McpError::new(ErrorCategory::LocalStorage)),
    }
}

fn invalid_arguments() -> McpError {
    McpError::new(ErrorCategory::ProviderValidation)
}

/// Enforces the MCP-local transport bound on a shared opaque identifier.
///
/// The shared `ItemInstanceId` and `CharacterId` types accept any string, so the
/// MCP layer bounds them to `1..=MAX_IDENTIFIER_LENGTH` characters to keep every
/// request finite. This changes no core identifier type; it only rejects empty
/// or over-long caller input before any service call.
fn validate_identifier(value: &str) -> Result<(), McpError> {
    let length = value.chars().count();
    if length == 0 || length > MAX_IDENTIFIER_LENGTH {
        return Err(invalid_arguments());
    }
    Ok(())
}

fn optional_term(value: Option<&str>) -> Result<Option<SearchTerm>, McpError> {
    value
        .map(SearchTerm::new)
        .transpose()
        .map_err(McpError::from)
}

fn character_view(character: &Character) -> CharacterView {
    CharacterView {
        id: character.id.as_str().to_owned(),
        name: character.name.clone(),
        class: character.class.clone(),
        race: character.race.clone(),
        gender: character.gender.clone(),
        light: character.light,
    }
}

fn definition_view(definition: &ItemDefinition) -> DefinitionView {
    DefinitionView {
        hash: definition.hash.value(),
        bucket_hash: definition.bucket_hash.map(|hash| hash.value()),
        name: definition.name.clone(),
        item_type: definition.item_type.clone(),
        tier: definition.tier.clone(),
        element: definition.element.clone(),
        tags: definition.tags.clone(),
        definition_version: definition.version.as_str().to_owned(),
    }
}

fn instance_view(instance: &ItemInstance) -> InstanceView {
    InstanceView {
        instance_id: instance.instance_id.as_str().to_owned(),
        item_hash: instance.item_hash.value(),
        location: location_view(&instance.location),
        quantity: instance.quantity,
        equipped: instance.equipped,
        locked: instance.locked,
        power: instance.power,
        perks: instance
            .perks
            .iter()
            .map(|perk| PerkView {
                perk_hash: perk.perk_hash.value(),
                active: perk.active,
                visible: perk.visible,
            })
            .collect(),
    }
}

fn location_view(location: &ItemLocation) -> LocationView {
    match location {
        ItemLocation::Vault => LocationView::Vault,
        ItemLocation::Character(character_id) => LocationView::Character {
            character_id: character_id.as_str().to_owned(),
        },
        // The opaque provider value is deliberately dropped so no provider text
        // crosses the MCP boundary.
        ItemLocation::Other(_) => LocationView::Other,
    }
}

fn metadata_view(metadata: &QueryMetadata) -> QueryMetadataView {
    QueryMetadataView {
        dataset: dataset_code(metadata.sync.dataset).to_owned(),
        source: source_code(metadata.sync.source).to_owned(),
        revision: metadata.sync.revision.value(),
        checked_at: metadata.sync.checked_at.seconds(),
        definition_version: metadata.sync.definition_version.as_str().to_owned(),
        freshness: freshness_code(metadata.freshness).to_owned(),
    }
}

fn dataset_code(dataset: Dataset) -> &'static str {
    match dataset {
        Dataset::ManifestDefinitions => "manifest_definitions",
        Dataset::ProfileInventory => "profile_inventory",
    }
}

fn source_code(source: SourceMarker) -> &'static str {
    match source {
        SourceMarker::Manifest => "manifest",
        SourceMarker::Profile => "profile",
    }
}

fn freshness_code(freshness: FreshnessStatus) -> &'static str {
    match freshness {
        FreshnessStatus::Missing => "missing",
        FreshnessStatus::Fresh => "fresh",
        FreshnessStatus::Stale => "stale",
        FreshnessStatus::InvalidSource => "invalid_source",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InventoryReadService, ListCharactersRequest, QueryReadService, inspect_item_output,
        list_characters_output, search_inventory_output,
    };
    use crate::schema::{
        InspectItemArgs, InspectItemOutput, InstanceQueryArgs, LocationView, MAX_IDENTIFIER_LENGTH,
        SearchInventoryArgs,
    };
    use destiny_app::test_support::{
        FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
        InMemorySearchRepository, QueuedProfileGateway,
    };
    use destiny_app::{
        AppError, Character, CharacterId, DefinitionReplacementInput, DefinitionRepository,
        DefinitionSearchQuery, DefinitionVersion, DestinyMembership, ErrorCategory,
        FreshnessPolicy, InstanceSearchQuery, InventoryRepository, InventorySnapshot,
        InventorySnapshotInput, ItemDefinition, ItemHash, ItemInstance, ItemInstanceId,
        ItemLocation, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS,
        MembershipId, MembershipType, PerkHash, PerkSummary, ProfileGateway, ProfileSyncService,
        QueryService, Resolution, SearchRepository, SourceMarker, SyncMetadata, Timestamp,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(10_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    const FRESH_CHECKED_AT: Timestamp = Timestamp::new(9_900);
    const STALE_CHECKED_AT: Timestamp = Timestamp::new(9_000);
    const DEFINITION_VERSION: &str = "definitions-v1";
    const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

    /// A narrow test-local composite exposing one repository for all three seams.
    #[derive(Clone, Debug, Default)]
    struct CompositeRepository {
        inventory: InMemoryInventoryRepository,
        search: InMemorySearchRepository,
        definitions: InMemoryDefinitionRepository,
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            let version = DefinitionVersion::new(DEFINITION_VERSION);
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions.clone()),
                definitions: InMemoryDefinitionRepository::new(version, definitions),
                calls: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        /// Commits one valid projection and mirrors its instances into search.
        ///
        /// This uses the inner repository directly, so seeding records no
        /// read-seam call.
        fn commit(&mut self, input: InventorySnapshotInput) {
            let snapshot = self
                .inventory
                .replace_current(input)
                .expect("valid seed snapshot");
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

    type AppService = QueryService<SharedGateway, CompositeRepository>;
    type McpService = QueryReadService<SharedGateway, CompositeRepository, FakeClock>;

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
    }

    fn app_service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        repository: CompositeRepository,
    ) -> AppService {
        QueryService::new(ProfileSyncService::new(
            SharedGateway(Rc::clone(shared)),
            repository,
        ))
    }

    fn mcp_service(queries: AppService) -> McpService {
        QueryReadService::new(queries, FakeClock::fixed(NOW), TTL)
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
            instance_id: ItemInstanceId::new(instance_id),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
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

    fn fresh_components(
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> (Rc<RefCell<QueuedProfileGateway>>, AppService) {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, characters, items));
        (Rc::clone(&shared), app_service(&shared, repository))
    }

    fn search_args(
        text: Option<&str>,
        tags: Vec<&str>,
        item_hashes: Option<Vec<i64>>,
        limit: usize,
    ) -> SearchInventoryArgs {
        SearchInventoryArgs {
            text: text.map(str::to_owned),
            item_type: None,
            tags: tags.into_iter().map(str::to_owned).collect(),
            limit,
            instances: item_hashes.map(|hashes| InstanceQueryArgs {
                item_hashes: Some(hashes),
                perk_hash: None,
                character_id: None,
                limit,
            }),
        }
    }

    fn instance_search_args(character_id: Option<&str>) -> SearchInventoryArgs {
        SearchInventoryArgs {
            text: None,
            item_type: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: character_id.map(str::to_owned),
                limit: MAX_SEARCH_LIMIT,
            }),
        }
    }

    #[test]
    fn definition_bounds_are_rejected_before_any_service_call() {
        for invalid_limit in [0, MAX_SEARCH_LIMIT + 1] {
            let error = search_args(None, Vec::new(), None, invalid_limit)
                .into_request()
                .expect_err("invalid limit must be rejected");
            assert_eq!(error.code(), "provider_validation");
        }

        let too_many_tags = vec!["tag"; MAX_TAG_FILTERS + 1];
        let error = search_args(None, too_many_tags, None, MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("too many tags must be rejected");
        assert_eq!(error.code(), "provider_validation");

        let overlong = "a".repeat(MAX_SEARCH_TEXT_LENGTH + 1);
        let error = search_args(Some(&overlong), Vec::new(), None, MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("overlong text must be rejected");
        assert_eq!(error.code(), "provider_validation");
        assert!(!error.message().contains(&overlong));
    }

    #[test]
    fn instance_candidate_bounds_are_rejected_before_any_service_call() {
        let over_large: Vec<i64> = (0..=MAX_HASH_FILTERS as i64).collect();
        let error = search_args(None, Vec::new(), Some(over_large), MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("over-large hash candidate set must be rejected");
        assert_eq!(error.code(), "provider_validation");

        // An empty candidate set is valid and keeps its fail-closed meaning.
        let request = search_args(None, Vec::new(), Some(Vec::new()), MAX_SEARCH_LIMIT)
            .into_request()
            .expect("empty candidate set is valid");
        assert!(request.instances.is_some());
    }

    #[test]
    fn invalid_identifiers_are_rejected_before_any_service_call() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, Vec::new(), Vec::new()));
        let calls = Rc::clone(&repository.calls);
        let _mcp = mcp_service(app_service(&shared, repository));

        let empty_instance = InspectItemArgs {
            instance_id: String::new(),
        }
        .into_request()
        .expect_err("empty instance identifier must be rejected");
        assert_eq!(empty_instance.code(), "provider_validation");

        let overlong_instance = "i".repeat(MAX_IDENTIFIER_LENGTH + 1);
        let error = InspectItemArgs {
            instance_id: overlong_instance,
        }
        .into_request()
        .expect_err("over-long instance identifier must be rejected");
        assert_eq!(error.code(), "provider_validation");

        let empty_character = instance_search_args(Some(""))
            .into_request()
            .expect_err("empty character identifier must be rejected");
        assert_eq!(empty_character.code(), "provider_validation");

        let overlong_character = "c".repeat(MAX_IDENTIFIER_LENGTH + 1);
        let error = instance_search_args(Some(&overlong_character))
            .into_request()
            .expect_err("over-long character identifier must be rejected");
        assert_eq!(error.code(), "provider_validation");
        assert!(!error.message().contains(&overlong_character));

        // The rejected arguments never reached the composed service: no
        // repository read and no gateway call happened.
        assert!(calls.borrow().is_empty());
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn identifier_boundary_is_inclusive() {
        let boundary = "i".repeat(MAX_IDENTIFIER_LENGTH);
        let inspect = InspectItemArgs {
            instance_id: boundary.clone(),
        }
        .into_request()
        .expect("boundary identifier is valid");
        assert_eq!(inspect.instance_id.as_str(), boundary);

        let search = instance_search_args(Some(&boundary))
            .into_request()
            .expect("boundary character identifier is valid");
        assert!(search.instances.is_some());
    }

    #[test]
    fn list_characters_parity_with_the_shared_application_result() {
        let characters = vec![character("warlock"), character("hunter")];
        let (shared, mut app) = fresh_components(characters.clone(), Vec::new());
        let app_result = app.list_characters(TTL, NOW).expect("application read");

        let (_shared, app_for_mcp) = fresh_components(characters, Vec::new());
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp
            .list_characters(&ListCharactersRequest)
            .expect("mcp read");

        assert_eq!(output, list_characters_output(&app_result));
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(output.characters.len(), 2);
        assert_eq!(output.characters[0].id, "hunter");
        assert_eq!(output.characters[1].id, "warlock");
        assert_eq!(output.metadata.dataset, "profile_inventory");
        assert_eq!(output.metadata.source, "profile");
        assert_eq!(output.metadata.freshness, "fresh");
        assert_eq!(output.metadata.revision, 1);
    }

    #[test]
    fn search_inventory_parity_with_the_shared_application_result() {
        let items = vec![
            instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)]),
            instance("inst-b", 2, ItemLocation::Vault, vec![perk(20)]),
        ];
        let args = SearchInventoryArgs {
            text: Some("gjallar".into()),
            item_type: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: Some(vec![1, 2]),
                perk_hash: None,
                character_id: None,
                limit: MAX_SEARCH_LIMIT,
            }),
        };

        let (_shared, mut app) = fresh_components(Vec::new(), items.clone());
        let app_result = app
            .search_inventory(
                &args.clone().into_request().expect("valid arguments"),
                TTL,
                NOW,
            )
            .expect("application search");

        let (_shared, app_for_mcp) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp
            .search_inventory(&args.into_request().expect("valid arguments"))
            .expect("mcp search");

        assert_eq!(output, search_inventory_output(&app_result));
        assert_eq!(
            output
                .candidates
                .iter()
                .map(|definition| definition.hash)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            output
                .hits
                .iter()
                .map(|hit| hit.instance.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["inst-a", "inst-b"]
        );
        // Every instance hit keeps its exact definition identity distinct.
        for hit in &output.hits {
            assert_eq!(hit.definition.hash, hit.instance.item_hash);
        }
    }

    #[test]
    fn inspect_item_parity_keeps_definition_and_instance_distinct() {
        let items = vec![instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)])];
        let request = InspectItemArgs {
            instance_id: "inst-a".into(),
        }
        .into_request()
        .expect("valid identifier");

        let (_shared, mut app) = fresh_components(Vec::new(), items.clone());
        let resolution = app
            .inspect_item(&request, TTL, NOW)
            .expect("application inspection");
        let expected = inspect_item_output(&resolution).expect("mapped inspection");

        let (_shared, app_for_mcp) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp.inspect_item(&request).expect("mcp inspection");

        assert_eq!(output, expected);
        match output {
            InspectItemOutput::Found(found) => {
                assert_eq!(found.definition.hash, 1);
                assert_eq!(found.instance.instance_id, "inst-a");
                assert_eq!(found.instance.item_hash, 1);
                assert_eq!(found.metadata.freshness, "fresh");
            }
            InspectItemOutput::Missing => panic!("expected a found inspection"),
        }
    }

    #[test]
    fn inspect_item_reports_missing_as_an_explicit_successful_outcome() {
        let (_shared, app) = fresh_components(Vec::new(), Vec::new());
        let mut mcp = mcp_service(app);

        let output = mcp
            .inspect_item(
                &InspectItemArgs {
                    instance_id: "absent".into(),
                }
                .into_request()
                .expect("valid identifier"),
            )
            .expect("missing is not an error");

        assert_eq!(output, InspectItemOutput::Missing);
    }

    #[test]
    fn ambiguous_instance_fails_closed_with_a_sanitized_error() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, Vec::new(), Vec::new()));
        let mut queries = app_service(&shared, repository);
        let duplicate = instance("dup", 1, ItemLocation::Vault, Vec::new());
        queries
            .repository_mut()
            .search
            .set_instances(vec![duplicate.clone(), duplicate]);
        let mut mcp = mcp_service(queries);

        let error = mcp
            .inspect_item(
                &InspectItemArgs {
                    instance_id: "dup".into(),
                }
                .into_request()
                .expect("valid identifier"),
            )
            .expect_err("ambiguous instance must fail closed");

        assert_eq!(error.code(), "local_storage");
        assert_eq!(error.message(), "local storage failed");
    }

    #[test]
    fn provider_failures_surface_only_sanitized_categories() {
        const PAYLOAD: &str = "raw-provider-payload-sentinel";
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            STALE_CHECKED_AT,
            vec![character("warlock")],
            vec![instance(PAYLOAD, 1, ItemLocation::Vault, Vec::new())],
        ));
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderValidation)));
        let mut mcp = mcp_service(app_service(&shared, repository));

        let error = mcp
            .list_characters(&ListCharactersRequest)
            .expect_err("stale refresh failure must fail the read");

        assert_eq!(error.code(), "provider_validation");
        assert_eq!(
            error.message(),
            AppError::new(ErrorCategory::ProviderValidation).to_string()
        );

        let rendered_error = error.to_string();
        let rendered_result =
            serde_json::to_string(&error.into_call_tool_result()).expect("error result serializes");
        for rendered in [rendered_error, rendered_result] {
            assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
            assert!(
                !rendered.contains(MEMBERSHIP_IDENTIFIER),
                "leaked membership identifier: {rendered}"
            );
        }
    }

    #[test]
    fn mcp_values_never_carry_raw_provider_material() {
        const PAYLOAD: &str = "raw-provider-payload-sentinel";
        let items = vec![instance(
            "inst-a",
            1,
            ItemLocation::other(PAYLOAD),
            Vec::new(),
        )];
        let (_shared, app) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app);

        let args = SearchInventoryArgs {
            text: None,
            item_type: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: None,
                limit: MAX_SEARCH_LIMIT,
            }),
        };
        let output = mcp
            .search_inventory(&args.into_request().expect("valid arguments"))
            .expect("search succeeds");

        assert_eq!(output.hits.len(), 1);
        assert_eq!(output.hits[0].instance.location, LocationView::Other);

        let rendered = serde_json::to_string(&output).expect("output serializes");
        assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
        assert!(
            !rendered.contains(MEMBERSHIP_IDENTIFIER),
            "leaked membership identifier: {rendered}"
        );
    }

    #[test]
    fn translation_is_deterministic_across_repeated_reads() {
        let items = vec![
            instance("inst-b", 2, ItemLocation::Vault, Vec::new()),
            instance("inst-a", 1, ItemLocation::Vault, Vec::new()),
        ];
        let (_shared, app) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app);

        let args = SearchInventoryArgs {
            text: None,
            item_type: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: None,
                limit: MAX_SEARCH_LIMIT,
            }),
        };
        let request = args.into_request().expect("valid arguments");

        let first = mcp.search_inventory(&request).expect("first read");
        let second = mcp.search_inventory(&request).expect("second read");

        assert_eq!(first, second);
        assert_eq!(
            first
                .hits
                .iter()
                .map(|hit| hit.instance.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["inst-a", "inst-b"]
        );
    }

    #[test]
    fn the_read_service_seam_is_object_safe_and_injectable() {
        let (_shared, app) = fresh_components(vec![character("warlock")], Vec::new());
        let mut service: Box<dyn InventoryReadService> = Box::new(mcp_service(app));

        let output = service
            .list_characters(&ListCharactersRequest)
            .expect("injected read succeeds");

        assert_eq!(output.characters.len(), 1);
        assert_eq!(output.characters[0].id, "warlock");
    }
}
