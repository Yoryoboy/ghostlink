// Reuse the provider-neutral application fakes for default-feature tests too.
#[cfg(feature = "test-support")]
use destiny_app::test_support;
#[cfg(not(feature = "test-support"))]
#[allow(dead_code)]
#[path = "../src/test_support.rs"]
mod test_support;

use destiny_app::{ProfileSyncService, QueryService};
use destiny_core::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use test_support::{
    InMemoryDefinitionRepository, InMemoryInventoryRepository, InMemorySearchRepository,
    QueuedProfileGateway,
};

#[derive(Default)]
struct Repository {
    inventory: InMemoryInventoryRepository,
    definitions: InMemoryDefinitionRepository,
    search: InMemorySearchRepository,
    lookup_error: Option<AppError>,
    read_error: Option<AppError>,
    disappear_after: Option<usize>,
    reads: Cell<usize>,
    corrupt: Option<InventorySnapshot>,
}
impl InventoryRepository for Repository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        self.reads.set(self.reads.get() + 1);
        if let Some(error) = &self.read_error {
            return Err(error.clone());
        }
        if self
            .disappear_after
            .is_some_and(|limit| self.reads.get() > limit)
        {
            return Ok(None);
        }
        if let Some(snapshot) = &self.corrupt {
            return Ok(Some(snapshot.clone()));
        }
        self.inventory.load_current()
    }
    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        self.inventory.replace_current(input)
    }
}
impl DefinitionRepository for Repository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.definitions.current_definition_version()
    }
    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        self.definitions.current_metadata()
    }
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        if let Some(error) = &self.lookup_error {
            return Err(error.clone());
        }
        self.definitions.lookup_definition(hash)
    }
    fn replace_definitions(
        &mut self,
        input: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.definitions.replace_definitions(input)
    }
}
impl SearchRepository for Repository {
    fn search_definitions(
        &self,
        query: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        self.search.search_definitions(query)
    }
    fn search_instances(&self, query: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
        self.search.search_instances(query)
    }
    fn lookup_instance(&self, id: &ItemInstanceId) -> Result<Resolution<ItemInstance>, AppError> {
        self.search.lookup_instance(id)
    }
}
impl StatDefinitionRepository for Repository {
    fn lookup_stat(&self, hash: u32) -> Result<Option<StatDefinition>, AppError> {
        self.definitions.lookup_stat(hash)
    }
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
        self.definitions.stat_hashes_by_name(name)
    }
    fn stat_definition_count(&self) -> Result<usize, AppError> {
        self.definitions.stat_definition_count()
    }
}
struct Gateway(Rc<RefCell<QueuedProfileGateway>>);
impl ProfileGateway for Gateway {
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
const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 10 };
fn character(id: &str, class: &str) -> Character {
    Character {
        id: CharacterId::new(id),
        name: None,
        class: Some(class.into()),
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    }
}
fn instance(id: &str, hash: i64, location: ItemLocation) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(hash),
        location,
        quantity: 1,
        equipped: false,
        locked: false,
        power: None,
        perks: vec![],
        plugs: vec![],
        stats: vec![],
        available_plugs: vec![],
    }
}
fn stack(hash: i64, quantity: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(hash),
        BucketHash::new(215593132),
        quantity,
        container,
    )
    .unwrap()
}
fn definition(hash: i64, name: &str) -> ItemDefinition {
    ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        name: name.into(),
        bucket_hash: Some(BucketHash::new(999)),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("test"),
    }
}
fn input() -> InventorySnapshotInput {
    let owner = CharacterId::new("a");
    InventorySnapshotInput {
        membership_id: MembershipId::new("12345"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("test"),
        checked_at: NOW,
        source: SourceMarker::Profile,
        characters: vec![character("b", "Warlock"), character("a", "Titan")],
        items: vec![
            instance("z", 1, ItemLocation::Postmaster(owner.clone())),
            instance("a", 2, ItemLocation::Postmaster(owner.clone())),
            instance("carried", 1, ItemLocation::Character(owner.clone())),
            instance("vault", 1, ItemLocation::Vault),
        ],
        stacks: vec![
            // Material and consumable observations, including equal duplicates.
            stack(3, 8, StackContainer::Postmaster(owner.clone())),
            stack(2, 2, StackContainer::Postmaster(owner.clone())),
            stack(3, 8, StackContainer::Postmaster(owner.clone())),
            stack(1, 4, StackContainer::CharacterInventory(owner)),
            stack(1, 4, StackContainer::ProfileInventory),
            stack(1, 0, StackContainer::ProfileCurrencies),
        ],
        profile_currencies_coverage: ProfileCurrenciesCoverage::Available,
        plug_set_entries: vec![],
    }
}
fn repository(input: InventorySnapshotInput) -> Repository {
    let mut repo = Repository {
        definitions: InMemoryDefinitionRepository::new(
            DefinitionVersion::new("test"),
            vec![definition(1, "Gear"), definition(2, "Material")],
        ),
        ..Repository::default()
    };
    repo.inventory.replace_current(input).unwrap();
    repo
}
type Service = QueryService<Gateway, Repository>;
fn service(repo: Repository) -> (Service, Rc<RefCell<QueuedProfileGateway>>) {
    let gateway = Rc::new(RefCell::new(QueuedProfileGateway::new()));
    (
        QueryService::new(ProfileSyncService::new(Gateway(gateway.clone()), repo)),
        gateway,
    )
}
fn membership(id: &str) -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new(id)).unwrap()
}
fn category(result: Result<PostmasterResult, AppError>, expected: ErrorCategory) {
    let error = result.unwrap_err();
    assert_eq!(error, AppError::new(expected));
    assert!(!format!("{error:?}").contains("synthetic"));
}

#[test]
fn postmaster_projects_instances_and_duplicate_stacks_without_other_sources() {
    let (mut query, gateway) = service(repository(input()));
    let result: PostmasterResult = query.postmaster(None, POLICY, NOW).unwrap();
    let ids: Vec<_> = result
        .characters
        .iter()
        .map(|c| c.character_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "b"]);
    let group = &result.characters[0];
    assert_eq!(group.class.as_deref(), Some("Titan"));
    let ids: Vec<_> = group
        .instances
        .iter()
        .map(|i| i.instance_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "z"]);
    assert_eq!(group.instances[1].name.as_deref(), Some("Gear"));
    assert_eq!(group.instances[1].item_hash, ItemHash::new(1));
    assert_eq!(group.instances[1].quantity, 1);
    assert_eq!(group.stacks.len(), 3);
    assert_eq!(group.stacks[0].item_hash, ItemHash::new(2));
    assert_eq!(group.stacks[0].name.as_deref(), Some("Material"));
    assert_eq!(group.stacks[1], group.stacks[2]);
    assert_eq!(group.stacks[1].name, None);
    assert_eq!(group.stacks[1].quantity, 8);
    assert_eq!(group.stacks[1].bucket_hash, BucketHash::new(215593132));
    assert!(result.characters[1].instances.is_empty());
    assert!(result.characters[1].stacks.is_empty());
    assert_eq!(
        result.profile_currencies_coverage,
        ProfileCurrenciesCoverage::Available
    );
    assert_eq!(result.metadata.sync.checked_at, NOW);
    assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(result.metadata.sync.source, SourceMarker::Profile);
    assert_eq!(result.metadata.sync.dataset, Dataset::ProfileInventory);
    assert_eq!(result.metadata.sync.definition_version.as_str(), "test");
    assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(gateway.borrow().membership_calls(), 0);
    assert_eq!(gateway.borrow().inventory_calls(), 0);
}

#[test]
fn postmaster_selector_exact_id_class_unknown_and_ambiguity() {
    let (mut query, _) = service(repository(input()));
    for selector in ["a", "tItAn"] {
        let result = query
            .postmaster(Some(&CharacterId::new(selector)), POLICY, NOW)
            .unwrap();
        assert_eq!(result.characters.len(), 1);
        assert_eq!(result.characters[0].character_id.as_str(), "a");
    }
    let empty = query
        .postmaster(Some(&CharacterId::new("Warlock")), POLICY, NOW)
        .unwrap();
    assert!(empty.characters[0].stacks.is_empty());
    category(
        query.postmaster(Some(&CharacterId::new("unknown")), POLICY, NOW),
        ErrorCategory::ProviderValidation,
    );
    let mut seed = input();
    seed.characters.push(character("c", "Titan"));
    seed.characters.push(character("Titan", "Hunter"));
    let (mut query, _) = service(repository(seed));
    category(
        query.postmaster(Some(&CharacterId::new("titan")), POLICY, NOW),
        ErrorCategory::AmbiguousMatch,
    );
    let exact = query
        .postmaster(Some(&CharacterId::new("Titan")), POLICY, NOW)
        .unwrap();
    assert_eq!(exact.characters[0].character_id.as_str(), "Titan");
}

#[test]
fn postmaster_invalid_selector_is_rejected_before_any_repository_or_gateway_read() {
    let (mut query, gateway) = service(Repository::default());
    for selector in [
        "".to_string(),
        "  ".into(),
        "synthetic\nvalue".into(),
        "x".repeat(MAX_SEARCH_TEXT_LENGTH + 1),
    ] {
        category(
            query.postmaster(Some(&CharacterId::new(selector)), POLICY, NOW),
            ErrorCategory::ProviderValidation,
        );
    }
    assert_eq!(query.repository().reads.get(), 0);
    assert_eq!(gateway.borrow().membership_calls(), 0);
    // The bound counts characters, consistent with shared query text bounds.
    let boundary = "é".repeat(MAX_SEARCH_TEXT_LENGTH);
    let mut seed = input();
    seed.characters.push(character(&boundary, "Hunter"));
    let (mut query, _) = service(repository(seed));
    let result = query
        .postmaster(Some(&CharacterId::new(boundary)), POLICY, NOW)
        .unwrap();
    assert_eq!(result.characters.len(), 1);
}

#[test]
fn postmaster_empty_groups_keep_currency_coverage_and_allow_absent_definitions() {
    for coverage in [
        ProfileCurrenciesCoverage::NotObserved,
        ProfileCurrenciesCoverage::Unavailable,
        ProfileCurrenciesCoverage::Available,
    ] {
        let mut seed = input();
        seed.items.clear();
        seed.stacks.clear();
        seed.profile_currencies_coverage = coverage;
        let mut repo = repository(seed);
        repo.definitions = InMemoryDefinitionRepository::default();
        let (mut query, _) = service(repo);
        let result = query.postmaster(None, POLICY, NOW).unwrap();
        assert_eq!(result.profile_currencies_coverage, coverage);
        assert_eq!(result.characters.len(), 2);
        assert!(
            result
                .characters
                .iter()
                .all(|c| c.instances.is_empty() && c.stacks.is_empty())
        );
    }
}

#[test]
fn postmaster_definition_readiness_missing_names_ambiguity_and_read_errors() {
    let mut repo = repository(input());
    repo.definitions = InMemoryDefinitionRepository::default();
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::DefinitionsUnavailable,
    );
    let mut repo = repository(input());
    repo.definitions = InMemoryDefinitionRepository::new(
        DefinitionVersion::new("test"),
        vec![definition(1, "One"), definition(1, "Two")],
    );
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
    let mut repo = repository(input());
    repo.lookup_error = Some(AppError::new(ErrorCategory::LocalStorage));
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
    let mut repo = repository(input());
    repo.definitions = repo
        .definitions
        .with_metadata_failure(AppError::new(ErrorCategory::LocalStorage));
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
}

#[test]
fn postmaster_refresh_uses_new_committed_metadata_and_never_falls_back_on_failure() {
    let (mut query, gateway) = service(repository(input()));
    let mut refreshed = input();
    refreshed.checked_at = Timestamp::new(120);
    refreshed.items.retain(|i| i.instance_id.as_str() != "z");
    gateway
        .borrow_mut()
        .queue_membership(Ok(membership("12345")));
    gateway.borrow_mut().queue_inventory(Ok(refreshed));
    let result = query.postmaster(None, POLICY, Timestamp::new(111)).unwrap();
    assert_eq!(result.characters[0].instances.len(), 1);
    assert_eq!(result.metadata.sync.checked_at, Timestamp::new(120));
    assert_eq!(result.metadata.sync.revision, SyncRevision::new(2));
    assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(gateway.borrow().inventory_calls(), 1);
    gateway
        .borrow_mut()
        .queue_membership(Ok(membership("12345")));
    gateway
        .borrow_mut()
        .queue_inventory(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
    category(
        query.postmaster(None, POLICY, Timestamp::new(140)),
        ErrorCategory::ProviderUnavailable,
    );
    let snapshot = query
        .repository()
        .inventory
        .load_current()
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.metadata.checked_at, Timestamp::new(120));
    assert_eq!(gateway.borrow().inventory_calls(), 2);
}

#[test]
fn postmaster_membership_mismatch_and_missing_postsync_snapshot_fail_closed() {
    let (mut query, gateway) = service(Repository::default());
    gateway
        .borrow_mut()
        .queue_membership(Ok(membership("67890")));
    gateway.borrow_mut().queue_inventory(Ok(input()));
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::ProviderValidation,
    );
    assert!(
        query
            .repository()
            .inventory
            .load_current()
            .unwrap()
            .is_none()
    );
    let mut repo = repository(input());
    repo.disappear_after = Some(1);
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
}

#[test]
fn postmaster_corrupt_ownership_equipped_instances_and_storage_fail_closed() {
    for equipped in [true, false] {
        let mut repo = repository(input());
        let mut snapshot = repo.inventory.load_current().unwrap().unwrap();
        if equipped {
            snapshot.items[0].equipped = true;
        } else {
            snapshot.items[0].location = ItemLocation::Postmaster(CharacterId::new("unowned"));
        }
        repo.corrupt = Some(snapshot);
        let (mut query, _) = service(repo);
        category(
            query.postmaster(None, POLICY, NOW),
            ErrorCategory::LocalStorage,
        );
    }
    let mut repo = repository(input());
    repo.read_error = Some(AppError::new(ErrorCategory::LocalStorage));
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
}

#[test]
fn postmaster_order_is_deterministic_without_coalescing_rows() {
    let seed = input();
    let mut reversed = seed.clone();
    reversed.characters.reverse();
    reversed.items.reverse();
    reversed.stacks.reverse();
    // Bypass fake canonicalization so the query itself must sort all arrays.
    let mut repo = repository(reversed.clone());
    let mut snapshot = repo.inventory.load_current().unwrap().unwrap();
    snapshot.characters = reversed.characters;
    snapshot.items = reversed.items;
    snapshot.stacks = reversed.stacks;
    repo.corrupt = Some(snapshot);
    let (mut left, _) = service(repository(seed));
    let (mut right, _) = service(repo);
    assert_eq!(
        left.postmaster(None, POLICY, NOW).unwrap(),
        right.postmaster(None, POLICY, NOW).unwrap()
    );
}

#[test]
fn postmaster_missing_instance_names_and_legacy_coverage_preserve_holdings() {
    let mut seed = input();
    seed.items[0].item_hash = ItemHash::new(404);
    seed.stacks
        .retain(|s| s.container() != &StackContainer::ProfileCurrencies);
    seed.profile_currencies_coverage = ProfileCurrenciesCoverage::NotObserved;
    let (mut query, _) = service(repository(seed));
    let result = query.postmaster(None, POLICY, NOW).unwrap();
    assert_eq!(result.characters[0].instances[1].name, None);
    assert_eq!(
        result.characters[0].instances[1].item_hash,
        ItemHash::new(404)
    );
    assert_eq!(result.characters[0].stacks.len(), 3);
    assert_eq!(
        result.profile_currencies_coverage,
        ProfileCurrenciesCoverage::NotObserved
    );
}

#[test]
fn postmaster_owner_filter_and_stack_order_preserve_runtime_bucket_and_quantities() {
    let mut seed = input();
    seed.items.push(instance(
        "other-owner",
        1,
        ItemLocation::Postmaster(CharacterId::new("b")),
    ));
    seed.stacks.push(stack(
        2,
        7,
        StackContainer::Postmaster(CharacterId::new("b")),
    ));
    seed.stacks.push(stack(
        2,
        1,
        StackContainer::Postmaster(CharacterId::new("a")),
    ));
    seed.stacks.push(
        StackObservation::new(
            ItemHash::new(2),
            BucketHash::new(10),
            9,
            StackContainer::Postmaster(CharacterId::new("a")),
        )
        .unwrap(),
    );
    let (mut query, _) = service(repository(seed));
    let all = query.postmaster(None, POLICY, NOW).unwrap();
    assert_eq!(
        all.characters[1].instances[0].instance_id.as_str(),
        "other-owner"
    );
    assert_eq!(all.characters[1].stacks[0].quantity, 7);
    let selected = query
        .postmaster(Some(&CharacterId::new("Titan")), POLICY, NOW)
        .unwrap();
    assert_eq!(selected.characters.len(), 1);
    assert_eq!(selected.characters[0], all.characters[0]);
    let rows: Vec<_> = selected.characters[0]
        .stacks
        .iter()
        .map(|s| (s.item_hash.value(), s.bucket_hash.value(), s.quantity))
        .collect();
    assert_eq!(
        rows,
        [
            (2, 10, 9),
            (2, 215593132, 1),
            (2, 215593132, 2),
            (3, 215593132, 8),
            (3, 215593132, 8)
        ]
    );
}

#[test]
fn postmaster_no_characters_is_empty_without_inferred_coverage() {
    let mut seed = input();
    seed.characters.clear();
    seed.items.clear();
    seed.stacks.clear();
    seed.profile_currencies_coverage = ProfileCurrenciesCoverage::Unavailable;
    let (mut query, _) = service(repository(seed));
    let result = query.postmaster(None, POLICY, NOW).unwrap();
    assert!(result.characters.is_empty());
    assert_eq!(
        result.profile_currencies_coverage,
        ProfileCurrenciesCoverage::Unavailable
    );
    category(
        query.postmaster(Some(&CharacterId::new("Titan")), POLICY, NOW),
        ErrorCategory::ProviderValidation,
    );
}

#[test]
fn postmaster_ambiguous_stack_definition_and_unowned_stack_are_not_silently_omitted() {
    let mut repo = repository(input());
    repo.definitions = InMemoryDefinitionRepository::new(
        DefinitionVersion::new("test"),
        vec![definition(3, "Consumable"), definition(3, "Conflict")],
    );
    let (mut query, _) = service(repo);
    category(
        query.postmaster(None, POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
    let mut repo = repository(input());
    let mut snapshot = repo.inventory.load_current().unwrap().unwrap();
    snapshot.stacks.push(stack(
        3,
        1,
        StackContainer::Postmaster(CharacterId::new("unowned")),
    ));
    repo.corrupt = Some(snapshot);
    let (mut query, _) = service(repo);
    category(
        query.postmaster(Some(&CharacterId::new("b")), POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
}

#[test]
fn postmaster_selected_empty_group_still_propagates_definition_metadata_errors() {
    let (mut query, _) = service(repository(input()));
    query.repository_mut().definitions = InMemoryDefinitionRepository::default();
    let result = query
        .postmaster(Some(&CharacterId::new("b")), POLICY, NOW)
        .unwrap();
    assert!(result.characters[0].instances.is_empty());
    query.repository_mut().definitions = InMemoryDefinitionRepository::default()
        .with_metadata_failure(AppError::new(ErrorCategory::LocalStorage));
    category(
        query.postmaster(Some(&CharacterId::new("b")), POLICY, NOW),
        ErrorCategory::LocalStorage,
    );
}
