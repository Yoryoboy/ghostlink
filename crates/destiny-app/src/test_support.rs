//! Offline, provider-neutral fakes for application contract tests.
//!
//! These types are test support only. They model normalized behavior and never
//! perform network, filesystem, serialization, or credential-provider work.

use destiny_core::{
    AccessCredential, AppError, AuthorizationCallback, AuthorizationState, BrowserAuthorization,
    CallbackReceiver, Clock, CredentialMaterial, CredentialStore, DefinitionRepository,
    DefinitionVersion, ErrorCategory, InventoryRepository, InventorySnapshot,
    InventorySnapshotInput, ItemDefinition, ItemHash, ManifestDefinitions, ManifestGateway, PlanId,
    PlanIdGenerator, ProfileGateway, Resolution, SnapshotMetadata, SyncRevision, Timestamp,
};
use std::collections::{HashSet, VecDeque};

#[derive(Debug)]
pub struct DeterministicRandom {
    states: VecDeque<AuthorizationState>,
}

impl DeterministicRandom {
    pub fn new(states: impl IntoIterator<Item = AuthorizationState>) -> Self {
        Self {
            states: states.into_iter().collect(),
        }
    }
}

impl destiny_core::SecureRandom for DeterministicRandom {
    fn generate_state(&mut self) -> Result<AuthorizationState, AppError> {
        self.states
            .pop_front()
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::LocalStorage))
    }
}

#[derive(Debug, Default)]
pub struct RecordingBrowser {
    pub requests: Vec<destiny_core::ports::AuthorizationRequest>,
}

impl BrowserAuthorization for RecordingBrowser {
    fn open(&mut self, request: destiny_core::ports::AuthorizationRequest) -> Result<(), AppError> {
        self.requests.push(request);
        Ok(())
    }
}

#[derive(Debug)]
pub struct FailingBrowser {
    pub error: AppError,
}

impl BrowserAuthorization for FailingBrowser {
    fn open(
        &mut self,
        _request: destiny_core::ports::AuthorizationRequest,
    ) -> Result<(), AppError> {
        Err(self.error)
    }
}

#[derive(Debug, Default)]
pub struct QueuedCallbackReceiver {
    pub callbacks: VecDeque<Result<AuthorizationCallback, AppError>>,
}

impl CallbackReceiver for QueuedCallbackReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        self.callbacks.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Debug, Default)]
pub struct QueuedTokenExchange {
    pub outcomes: VecDeque<Result<AccessCredential, AppError>>,
}

impl destiny_core::PublicTokenExchange for QueuedTokenExchange {
    fn exchange(
        &mut self,
        _code: destiny_core::AuthorizationCode,
    ) -> Result<AccessCredential, AppError> {
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub struct FakeClock {
    now: Timestamp,
}

impl FakeClock {
    pub const fn fixed(now: Timestamp) -> Self {
        Self { now }
    }
    pub const fn now_value(&self) -> Timestamp {
        self.now
    }
    pub fn advance(&mut self, seconds: u64) -> Result<(), AppError> {
        let next = self
            .now
            .seconds()
            .checked_add(seconds)
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::ClockOverflow))?;
        self.now = Timestamp::new(next);
        Ok(())
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(self.now)
    }
}

#[derive(Clone, Debug)]
pub struct DeterministicPlanIds {
    next: u64,
}

impl DeterministicPlanIds {
    pub const fn starting_at(next: u64) -> Self {
        Self { next }
    }
}

impl PlanIdGenerator for DeterministicPlanIds {
    fn next_plan_id(&mut self) -> Result<PlanId, AppError> {
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::PlanIdExhausted))?;
        let id = PlanId::new(format!("plan-{}", self.next));
        self.next = next;
        Ok(id)
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryDefinitionRepository {
    version: Option<DefinitionVersion>,
    definitions: Vec<ItemDefinition>,
    publication_failure: Option<AppError>,
    publication_attempts: usize,
}

impl InMemoryDefinitionRepository {
    /// Seeds arbitrary state for read-path contract tests.
    ///
    /// This bypasses publication validation on purpose so lookup behavior such
    /// as ambiguous matches stays directly testable. Publication always
    /// validates the complete set before mutating state.
    pub fn new(version: DefinitionVersion, definitions: Vec<ItemDefinition>) -> Self {
        Self {
            version: Some(version),
            definitions,
            ..Self::default()
        }
    }

    /// Injects a sanitized publication failure; reads keep serving prior state.
    pub fn with_publication_failure(mut self, error: AppError) -> Self {
        self.publication_failure = Some(error);
        self
    }

    /// Counts publication attempts, including rejected ones.
    pub const fn publication_attempts(&self) -> usize {
        self.publication_attempts
    }

    /// Counts the definitions currently visible in the active set.
    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    /// A publishable set is non-empty, version-consistent, and duplicate-free.
    fn validate_publication(definitions: &ManifestDefinitions) -> Result<(), AppError> {
        if definitions.definitions.is_empty() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let mut seen = HashSet::with_capacity(definitions.definitions.len());
        for definition in &definitions.definitions {
            if definition.version != definitions.version || !seen.insert(definition.hash) {
                return Err(AppError::new(ErrorCategory::ProviderValidation));
            }
        }
        Ok(())
    }
}

impl DefinitionRepository for InMemoryDefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.version
            .clone()
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))
    }

    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        let matches = self
            .definitions
            .iter()
            .filter(|definition| definition.hash == hash)
            .cloned()
            .collect::<Vec<_>>();
        Ok(match matches.as_slice() {
            [] => Resolution::Missing,
            [definition] => Resolution::Unique(definition.clone()),
            _ => Resolution::Ambiguous(matches),
        })
    }

    fn replace_definitions(&mut self, definitions: ManifestDefinitions) -> Result<(), AppError> {
        self.publication_attempts += 1;
        if let Some(error) = self.publication_failure {
            return Err(error);
        }
        Self::validate_publication(&definitions)?;
        self.version = Some(definitions.version);
        self.definitions = definitions.definitions;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryInventoryRepository {
    current: Option<InventorySnapshot>,
    previous_input: Option<InventorySnapshotInput>,
}

impl InMemoryInventoryRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs an overflow boundary state for repository contract tests.
    pub fn with_revision_for_test(input: InventorySnapshotInput, revision: SyncRevision) -> Self {
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id.clone(),
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version.clone(),
            },
            characters: input.characters.clone(),
            items: input.items.clone(),
        };
        Self {
            current: Some(snapshot),
            previous_input: Some(input),
        }
    }
}

impl InventoryRepository for InMemoryInventoryRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        Ok(self.current.clone())
    }

    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        let revision = match (self.current.as_ref(), self.previous_input.as_ref()) {
            (Some(current), Some(previous)) if previous == &input => current.metadata.revision,
            (Some(current), _) => SyncRevision::new(
                current
                    .metadata
                    .revision
                    .value()
                    .checked_add(1)
                    .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::LocalStorage))?,
            ),
            (None, _) => SyncRevision::new(1),
        };
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id,
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version,
            },
            characters: input.characters,
            items: input.items,
        };
        self.current = Some(snapshot.clone());
        self.previous_input = Some(InventorySnapshotInput::with_metadata(&snapshot));
        Ok(snapshot)
    }
}

#[derive(Debug, Default)]
pub struct QueuedProfileGateway {
    outcomes: VecDeque<Result<InventorySnapshotInput, AppError>>,
    calls: usize,
}

impl QueuedProfileGateway {
    pub fn new() -> Self {
        Self {
            outcomes: VecDeque::new(),
            calls: 0,
        }
    }
    pub fn queue(&mut self, outcome: Result<InventorySnapshotInput, AppError>) {
        self.outcomes.push_back(outcome);
    }
    pub const fn calls(&self) -> usize {
        self.calls
    }
}

impl ProfileGateway for QueuedProfileGateway {
    fn load_inventory(&mut self) -> Result<InventorySnapshotInput, AppError> {
        self.calls += 1;
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Debug, Default)]
pub struct QueuedManifestGateway {
    outcomes: VecDeque<Result<ManifestDefinitions, AppError>>,
    calls: usize,
}

impl QueuedManifestGateway {
    pub fn new() -> Self {
        Self {
            outcomes: VecDeque::new(),
            calls: 0,
        }
    }
    pub fn queue(&mut self, outcome: Result<ManifestDefinitions, AppError>) {
        self.outcomes.push_back(outcome);
    }
    pub const fn calls(&self) -> usize {
        self.calls
    }
}

impl ManifestGateway for QueuedManifestGateway {
    fn load_definitions(&mut self) -> Result<ManifestDefinitions, AppError> {
        self.calls += 1;
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryCredentialStore {
    material: Option<CredentialMaterial>,
}

impl InMemoryCredentialStore {
    pub fn contains_material(&self) -> bool {
        self.material.is_some()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn save(&mut self, material: CredentialMaterial) -> Result<(), AppError> {
        self.material = Some(material);
        Ok(())
    }
    fn load(&self) -> Result<Option<CredentialMaterial>, AppError> {
        Ok(self.material.clone())
    }
    fn delete(&mut self) -> Result<(), AppError> {
        self.material = None;
        Ok(())
    }
}

#[derive(Debug)]
pub struct FailingCredentialStore {
    pub error: AppError,
}

impl CredentialStore for FailingCredentialStore {
    fn save(&mut self, _material: CredentialMaterial) -> Result<(), AppError> {
        Err(self.error)
    }

    fn load(&self) -> Result<Option<CredentialMaterial>, AppError> {
        Err(self.error)
    }

    fn delete(&mut self) -> Result<(), AppError> {
        Err(self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        Character, ErrorCategory, ItemInstance, ItemLocation, MembershipId, MembershipType,
    };

    type RevisionMutation = (&'static str, fn(&mut InventorySnapshotInput));

    fn input(quantity: u32) -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(1),
            definition_version: DefinitionVersion::new("definitions-v1"),
            characters: Vec::<Character>::new(),
            items: vec![ItemInstance {
                instance_id: "instance".into(),
                item_hash: ItemHash::new(7),
                location: ItemLocation::Vault,
                quantity,
                equipped: false,
                locked: false,
                power: None,
            }],
        }
    }

    fn item_definition(hash: i64, name: &str, version: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(version),
        }
    }

    fn definition_set(version: &str, definitions: Vec<ItemDefinition>) -> ManifestDefinitions {
        ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions,
        }
    }

    #[test]
    fn clock_and_ids_are_deterministic() {
        let mut clock = FakeClock::fixed(Timestamp::new(10));
        assert_eq!(clock.now().unwrap(), Timestamp::new(10));
        clock.advance(5).unwrap();
        assert_eq!(clock.now().unwrap(), Timestamp::new(15));
        let mut ids = DeterministicPlanIds::starting_at(4);
        assert_eq!(ids.next_plan_id().unwrap().as_str(), "plan-4");
        assert_eq!(ids.next_plan_id().unwrap().as_str(), "plan-5");
    }

    #[test]
    fn inventory_revisions_retain_then_advance() {
        let mut repo = InMemoryInventoryRepository::new();
        assert_eq!(
            repo.replace_current(input(1))
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
        assert_eq!(
            repo.replace_current(input(1))
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
        assert_eq!(
            repo.replace_current(input(2))
                .unwrap()
                .metadata
                .revision
                .value(),
            2
        );
    }

    #[test]
    fn every_normalized_field_participates_in_revision_equality() {
        let base = InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(1),
            definition_version: DefinitionVersion::new("definitions-v1"),
            characters: vec![
                Character {
                    id: "character-1".into(),
                    name: Some("name-1".into()),
                    class: Some("class-1".into()),
                    race: Some("race-1".into()),
                    gender: Some("gender-1".into()),
                    light: Some(100),
                },
                Character {
                    id: "character-2".into(),
                    name: Some("name-2".into()),
                    class: Some("class-2".into()),
                    race: Some("race-2".into()),
                    gender: Some("gender-2".into()),
                    light: Some(200),
                },
            ],
            items: vec![
                ItemInstance {
                    instance_id: "instance-1".into(),
                    item_hash: ItemHash::new(7),
                    location: ItemLocation::Vault,
                    quantity: 1,
                    equipped: false,
                    locked: false,
                    power: Some(100),
                },
                ItemInstance {
                    instance_id: "instance-2".into(),
                    item_hash: ItemHash::new(8),
                    location: ItemLocation::Other("postmaster".into()),
                    quantity: 2,
                    equipped: true,
                    locked: true,
                    power: Some(200),
                },
            ],
        };

        let mutations: [RevisionMutation; 20] = [
            ("membership_id", |value| {
                value.membership_id = MembershipId::new("membership-2");
            }),
            ("membership_type", |value| {
                value.membership_type = MembershipType::new(2);
            }),
            ("definition_version", |value| {
                value.definition_version = DefinitionVersion::new("definitions-v2");
            }),
            ("character.id", |value| {
                value.characters[0].id = "character-1-changed".into();
            }),
            ("character.name", |value| {
                value.characters[0].name = Some("name-1-changed".into());
            }),
            ("character.class", |value| {
                value.characters[0].class = Some("class-1-changed".into());
            }),
            ("character.race", |value| {
                value.characters[0].race = Some("race-1-changed".into());
            }),
            ("character.gender", |value| {
                value.characters[0].gender = Some("gender-1-changed".into());
            }),
            ("character.light", |value| {
                value.characters[0].light = Some(101);
            }),
            ("item.instance_id", |value| {
                value.items[0].instance_id = "instance-1-changed".into();
            }),
            ("item.item_hash", |value| {
                value.items[0].item_hash = ItemHash::new(9);
            }),
            ("item.location", |value| {
                value.items[0].location = ItemLocation::Character("character-1".into());
            }),
            ("item.quantity", |value| {
                value.items[0].quantity = 3;
            }),
            ("item.equipped", |value| {
                value.items[0].equipped = true;
            }),
            ("item.locked", |value| {
                value.items[0].locked = true;
            }),
            ("item.power", |value| {
                value.items[0].power = Some(101);
            }),
            ("characters.membership", |value| {
                value.characters.pop();
            }),
            ("characters.order", |value| {
                value.characters.swap(0, 1);
            }),
            ("items.membership", |value| {
                value.items.pop();
            }),
            ("items.order", |value| {
                value.items.swap(0, 1);
            }),
        ];

        for (field, mutate) in mutations {
            let mut repo = InMemoryInventoryRepository::new();
            repo.replace_current(base.clone()).unwrap();
            let mut changed = base.clone();
            mutate(&mut changed);

            assert_eq!(
                repo.replace_current(changed)
                    .unwrap()
                    .metadata
                    .revision
                    .value(),
                2,
                "changing {field} must advance the revision"
            );
        }
    }

    #[test]
    fn revision_overflow_is_atomic() {
        let original = input(1);
        let mut repo = InMemoryInventoryRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );
        let changed = input(2);
        assert_eq!(
            repo.replace_current(changed),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            repo.load_current()
                .unwrap()
                .unwrap()
                .metadata
                .revision
                .value(),
            u64::MAX
        );
        assert_eq!(
            repo.replace_current(original)
                .unwrap()
                .metadata
                .revision
                .value(),
            u64::MAX
        );
    }

    #[test]
    fn clock_and_plan_ids_report_exhaustion_without_wrap() {
        let mut clock = FakeClock::fixed(Timestamp::new(u64::MAX));
        assert_eq!(
            clock.advance(1),
            Err(AppError::new(ErrorCategory::ClockOverflow))
        );
        assert_eq!(clock.now().unwrap().seconds(), u64::MAX);

        let mut ids = DeterministicPlanIds::starting_at(u64::MAX);
        assert_eq!(
            ids.next_plan_id(),
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        );
        assert_eq!(
            ids.next_plan_id(),
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        );
    }

    #[test]
    fn gateways_sequence_queued_failures_without_remote_side_effects() {
        let mut gateway = QueuedProfileGateway::new();
        gateway.queue(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        assert_eq!(
            gateway.load_inventory().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(gateway.calls(), 1);
        assert_eq!(
            gateway.load_inventory().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(gateway.calls(), 2);
    }

    #[test]
    fn repositories_and_gateways_cover_contract_outcomes() {
        let hash = ItemHash::new(7);
        let definition = ItemDefinition {
            hash,
            bucket_hash: None,
            name: "Item".into(),
            item_type: None,
            tier: None,
            element: None,
            tags: Vec::new(),
            version: DefinitionVersion::new("definitions-v1"),
        };
        let repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("definitions-v1"),
            vec![definition.clone(), definition.clone()],
        );
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "definitions-v1"
        );
        assert!(matches!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            Resolution::Missing
        ));
        assert!(matches!(
            repository.lookup_definition(hash).unwrap(),
            Resolution::Ambiguous(_)
        ));
        let unique = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("definitions-v1"),
            vec![definition],
        );
        assert!(matches!(
            unique.lookup_definition(hash).unwrap(),
            Resolution::Unique(_)
        ));

        let mut manifest = QueuedManifestGateway::new();
        let definitions = ManifestDefinitions {
            version: DefinitionVersion::new("v1"),
            definitions: Vec::new(),
        };
        manifest.queue(Ok(definitions.clone()));
        manifest.queue(Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(manifest.load_definitions().unwrap(), definitions);
        assert_eq!(
            manifest.load_definitions().unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(
            manifest.load_definitions().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(manifest.calls(), 3);

        let mut profile = QueuedProfileGateway::new();
        profile.queue(Ok(input(1)));
        profile.queue(Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(profile.load_inventory().unwrap(), input(1));
        assert_eq!(
            profile.load_inventory().unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(
            profile.load_inventory().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(profile.calls(), 3);

        let mut inventory = InMemoryInventoryRepository::new();
        assert_eq!(inventory.load_current().unwrap(), None);
        inventory.replace_current(input(1)).unwrap();
        assert_eq!(
            inventory
                .load_current()
                .unwrap()
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
    }

    #[test]
    fn definition_publication_replaces_the_complete_set() {
        let mut repository = InMemoryDefinitionRepository::default();
        assert_eq!(
            repository.current_definition_version(),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(repository.definition_count(), 0);

        repository
            .replace_definitions(definition_set(
                "manifest-v1",
                vec![
                    item_definition(7, "Alpha", "manifest-v1"),
                    item_definition(8, "Beta", "manifest-v1"),
                ],
            ))
            .unwrap();
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 2);

        repository
            .replace_definitions(definition_set(
                "manifest-v2",
                vec![item_definition(9, "Gamma", "manifest-v2")],
            ))
            .unwrap();
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v2"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Unique(item_definition(9, "Gamma", "manifest-v2"))
        );
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_is_idempotent_for_an_equal_normalized_set() {
        let mut repository = InMemoryDefinitionRepository::default();
        let set = definition_set(
            "manifest-v1",
            vec![
                item_definition(7, "Alpha", "manifest-v1"),
                item_definition(8, "Beta", "manifest-v1"),
            ],
        );

        repository.replace_definitions(set.clone()).unwrap();
        let version = repository.current_definition_version().unwrap();
        let alpha = repository.lookup_definition(ItemHash::new(7)).unwrap();
        let beta = repository.lookup_definition(ItemHash::new(8)).unwrap();

        repository.replace_definitions(set).unwrap();

        assert_eq!(repository.current_definition_version().unwrap(), version);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            alpha
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            beta
        );
        assert_eq!(repository.definition_count(), 2);
        assert!(matches!(alpha, Resolution::Unique(_)));
        assert!(matches!(beta, Resolution::Unique(_)));
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_rejects_duplicate_hashes_before_mutation() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let error = repository.replace_definitions(definition_set(
            "manifest-v2",
            vec![
                item_definition(7, "Alpha", "manifest-v2"),
                item_definition(7, "Alpha duplicate", "manifest-v2"),
            ],
        ));

        assert_eq!(error, Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(repository.publication_attempts(), 1);
    }

    #[test]
    fn definition_publication_rejects_mixed_versions_and_empty_sets_before_mutation() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let candidates = [
            definition_set(
                "manifest-v2",
                vec![
                    item_definition(7, "Alpha", "manifest-v2"),
                    item_definition(8, "Beta", "manifest-v1"),
                ],
            ),
            definition_set("manifest-v2", Vec::new()),
        ];

        for candidate in candidates {
            assert_eq!(
                repository.replace_definitions(candidate),
                Err(AppError::new(ErrorCategory::ProviderValidation))
            );
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                "manifest-v1"
            );
            assert_eq!(repository.definition_count(), 1);
        }
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn injected_publication_failure_preserves_prior_state() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        )
        .with_publication_failure(AppError::new(ErrorCategory::ProviderUnavailable));

        let error = repository.replace_definitions(definition_set(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        ));

        assert_eq!(
            error,
            Err(AppError::new(ErrorCategory::ProviderUnavailable))
        );
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(repository.publication_attempts(), 1);
    }

    #[test]
    fn published_lookup_and_version_reads_are_deterministic() {
        let alpha = item_definition(7, "Alpha", "manifest-v1");
        let beta = item_definition(7, "Beta", "manifest-v1");
        let repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![alpha.clone(), beta.clone()],
        );

        for _ in 0..3 {
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                "manifest-v1"
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(7)).unwrap(),
                Resolution::Ambiguous(vec![alpha.clone(), beta.clone()])
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(99)).unwrap(),
                Resolution::Missing
            );
        }
    }

    #[test]
    fn credential_material_is_redacted_and_never_displayable() {
        let material = CredentialMaterial::from_bytes(b"secret");
        assert_eq!(format!("{material:?}"), "CredentialMaterial");
        let mut store = InMemoryCredentialStore::default();
        store.save(material).unwrap();
        assert!(store.contains_material());
        assert_eq!(store.load().unwrap().unwrap().expose_secret(), b"secret");
        store.delete().unwrap();
        assert!(!store.contains_material());
    }
}
