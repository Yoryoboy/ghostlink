//! Offline, provider-neutral fakes for application contract tests.
//!
//! These types are test support only. They model normalized behavior and never
//! perform network, filesystem, serialization, or credential-provider work.

use destiny_core::{
    AccessCredential, AppError, AuthorizationCallback, AuthorizationState, BrowserAuthorization,
    CallbackReceiver, Clock, CredentialMaterial, CredentialStore, Dataset,
    DefinitionReplacementInput, DefinitionRepository, DefinitionVersion, ErrorCategory,
    InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemDefinition, ItemHash,
    ManifestAcquisition, ManifestDefinitions, ManifestGateway, PlanId, PlanIdGenerator,
    ProfileGateway, Resolution, SnapshotMetadata, SourceMarker, SyncMetadata, SyncRevision,
    Timestamp,
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
    metadata: Option<SyncMetadata>,
    previous_input: Option<DefinitionReplacementInput>,
    publication_failure: Option<AppError>,
    publication_attempts: usize,
}

impl InMemoryDefinitionRepository {
    /// Seeds arbitrary state for read-path contract tests.
    ///
    /// This bypasses publication validation on purpose so lookup behavior such
    /// as ambiguous matches stays directly testable. Publication always
    /// validates the complete set before mutating state, and the seeded state
    /// carries no committed metadata until the first successful publication.
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

    /// Constructs a committed overflow boundary state for repository contract tests.
    pub fn with_revision_for_test(
        replacement: DefinitionReplacementInput,
        revision: SyncRevision,
    ) -> Self {
        let metadata = SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: replacement.checked_at,
            source: replacement.source,
            revision,
            definition_version: replacement.definitions.version.clone(),
        };
        Self {
            version: Some(replacement.definitions.version.clone()),
            definitions: replacement.definitions.definitions.clone(),
            metadata: Some(metadata),
            previous_input: Some(replacement),
            ..Self::default()
        }
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

    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        Ok(self.metadata.clone())
    }

    fn replace_definitions(
        &mut self,
        replacement: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.publication_attempts += 1;
        if let Some(error) = self.publication_failure {
            return Err(error);
        }
        if replacement.source != SourceMarker::Manifest {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Self::validate_publication(&replacement.definitions)?;

        // Revision identity is the normalized definition content alone. A later
        // `checked_at` refreshes committed metadata but never advances the
        // revision; a fingerprint would collide, so content equality is the only
        // retention authority here.
        let revision = match self.previous_input.as_ref() {
            Some(previous) if previous.content_eq(&replacement) => self
                .metadata
                .as_ref()
                .map(|metadata| metadata.revision)
                .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
            _ => match self.metadata.as_ref() {
                Some(metadata) => SyncRevision::new(
                    metadata
                        .revision
                        .value()
                        .checked_add(1)
                        .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
                ),
                None => SyncRevision::new(1),
            },
        };

        let metadata = SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: replacement.checked_at,
            source: replacement.source,
            revision,
            definition_version: replacement.definitions.version.clone(),
        };
        self.version = Some(replacement.definitions.version.clone());
        self.definitions = replacement.definitions.definitions.clone();
        self.metadata = Some(metadata.clone());
        self.previous_input = Some(replacement);
        Ok(metadata)
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
    pub fn with_revision_for_test(
        mut input: InventorySnapshotInput,
        revision: SyncRevision,
    ) -> Self {
        input
            .characters
            .sort_by(|left, right| left.id.cmp(&right.id));
        input
            .items
            .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id.clone(),
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version.clone(),
                checked_at: input.checked_at,
                source: input.source,
            },
            characters: input.characters.clone(),
            items: input.items.clone(),
        };
        Self {
            current: Some(snapshot),
            previous_input: Some(input),
        }
    }

    /// Exposes the committed snapshot metadata, if any, without cloning the projection.
    pub fn snapshot_metadata(&self) -> Result<Option<SnapshotMetadata>, AppError> {
        Ok(self.load_current()?.map(|snapshot| snapshot.metadata))
    }
}

impl InventoryRepository for InMemoryInventoryRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        Ok(self.current.clone())
    }

    fn replace_current(
        &mut self,
        mut input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        // Provenance and structural validity are validated before any mutation so
        // a rejected replacement leaves the prior projection fully readable.
        if input.source != SourceMarker::Profile {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        if !input.is_structurally_valid() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        // Characters and items are unordered sets, so the committed and returned
        // vectors are canonicalized by identifier. Revision identity is the
        // normalized projection content through the shared `content_eq`:
        // freshness metadata (`checked_at`, `source`) never advances a revision,
        // but every successful replacement commits the new `checked_at`.
        input
            .characters
            .sort_by(|left, right| left.id.cmp(&right.id));
        input
            .items
            .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let revision = match (self.current.as_ref(), self.previous_input.as_ref()) {
            (Some(current), Some(previous)) if previous.content_eq(&input) => {
                current.metadata.revision
            }
            (Some(current), _) => SyncRevision::new(
                current
                    .metadata
                    .revision
                    .value()
                    .checked_add(1)
                    .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
            ),
            (None, _) => SyncRevision::new(1),
        };
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id,
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version,
                checked_at: input.checked_at,
                source: input.source,
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
    outcomes: VecDeque<Result<ManifestAcquisition, AppError>>,
    calls: usize,
}

impl QueuedManifestGateway {
    pub fn new() -> Self {
        Self {
            outcomes: VecDeque::new(),
            calls: 0,
        }
    }
    pub fn queue(&mut self, outcome: Result<ManifestAcquisition, AppError>) {
        self.outcomes.push_back(outcome);
    }
    pub const fn calls(&self) -> usize {
        self.calls
    }
}

impl ManifestGateway for QueuedManifestGateway {
    fn load_definitions(&mut self) -> Result<ManifestAcquisition, AppError> {
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
            checked_at: Timestamp::new(200),
            source: SourceMarker::Profile,
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

    fn multi_input() -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(1),
            definition_version: DefinitionVersion::new("definitions-v1"),
            checked_at: Timestamp::new(200),
            source: SourceMarker::Profile,
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
        }
    }

    fn character_ids(characters: &[Character]) -> Vec<&str> {
        characters
            .iter()
            .map(|character| character.id.as_str())
            .collect()
    }

    fn instance_ids(items: &[ItemInstance]) -> Vec<&str> {
        items.iter().map(|item| item.instance_id.as_str()).collect()
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

    fn replacement(version: &str, definitions: Vec<ItemDefinition>) -> DefinitionReplacementInput {
        DefinitionReplacementInput {
            definitions: definition_set(version, definitions),
            checked_at: Timestamp::new(200),
            source: SourceMarker::Manifest,
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
        let base = multi_input();

        let mutations: [RevisionMutation; 18] = [
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
            ("items.membership", |value| {
                value.items.pop();
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
    fn unchanged_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
        let mut repo = InMemoryInventoryRepository::new();
        let first = repo.replace_current(input(1)).unwrap();
        assert_eq!(first.metadata.revision.value(), 1);
        assert_eq!(first.metadata.checked_at, Timestamp::new(200));

        let mut refreshed = input(1);
        refreshed.checked_at = Timestamp::new(9_000);
        let second = repo.replace_current(refreshed).unwrap();

        assert_eq!(second.metadata.revision.value(), 1);
        assert_eq!(second.metadata.checked_at, Timestamp::new(9_000));
        assert_eq!(second.metadata.source, SourceMarker::Profile);
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(second.metadata));
    }

    #[test]
    fn inventory_overflow_requires_a_content_change() {
        let original = input(1);
        let mut repo = InMemoryInventoryRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );

        // An unchanged refresh at a later timestamp must not overflow; it
        // retains the revision and refreshes the committed timestamp.
        let mut refreshed = original.clone();
        refreshed.checked_at = Timestamp::new(9_000);
        let retained = repo.replace_current(refreshed).unwrap();
        assert_eq!(retained.metadata.revision.value(), u64::MAX);
        assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));

        // A genuine content change at the boundary fails atomically.
        assert_eq!(
            repo.replace_current(input(2)),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(repo.load_current().unwrap().unwrap(), retained);
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
    fn invalid_source_is_rejected_and_preserves_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut mismatched = input(2);
        mismatched.source = SourceMarker::Manifest;
        assert_eq!(
            repo.replace_current(mismatched),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn character_and_item_permutations_retain_revision_and_canonical_order() {
        let mut repo = InMemoryInventoryRepository::new();
        let first = repo.replace_current(multi_input()).unwrap();
        assert_eq!(first.metadata.revision.value(), 1);
        assert_eq!(
            character_ids(&first.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&first.items), ["instance-1", "instance-2"]);

        let mut permuted = multi_input();
        permuted.characters.reverse();
        permuted.items.reverse();
        let second = repo.replace_current(permuted).unwrap();

        assert_eq!(second.metadata.revision.value(), 1);
        assert_eq!(
            character_ids(&second.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&second.items), ["instance-1", "instance-2"]);
        assert_eq!(repo.load_current().unwrap(), Some(second));
    }

    #[test]
    fn permuted_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
        let mut repo = InMemoryInventoryRepository::new();
        repo.replace_current(multi_input()).unwrap();

        let mut permuted = multi_input();
        permuted.characters.reverse();
        permuted.items.reverse();
        permuted.checked_at = Timestamp::new(9_000);
        let refreshed = repo.replace_current(permuted).unwrap();

        assert_eq!(refreshed.metadata.revision.value(), 1);
        assert_eq!(refreshed.metadata.checked_at, Timestamp::new(9_000));
        assert_eq!(
            character_ids(&refreshed.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&refreshed.items), ["instance-1", "instance-2"]);
    }

    #[test]
    fn snapshot_input_structural_validity_covers_emptiness_duplicates_and_locations() {
        let mut candidate = multi_input();
        assert!(candidate.is_structurally_valid());

        candidate.characters.clear();
        candidate.items.clear();
        assert!(candidate.is_structurally_valid());

        let mut duplicate_characters = multi_input();
        duplicate_characters
            .characters
            .push(duplicate_characters.characters[0].clone());
        assert!(!duplicate_characters.is_structurally_valid());

        let mut duplicate_items = multi_input();
        duplicate_items.items.push(duplicate_items.items[0].clone());
        assert!(!duplicate_items.is_structurally_valid());

        let mut dangling = multi_input();
        dangling.items[0].location = ItemLocation::Character("missing-character".into());
        assert!(!dangling.is_structurally_valid());
    }

    #[test]
    fn duplicate_identifiers_are_rejected_and_preserve_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut duplicate_characters = multi_input();
        duplicate_characters
            .characters
            .push(duplicate_characters.characters[0].clone());
        assert_eq!(
            repo.replace_current(duplicate_characters),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        let mut duplicate_items = multi_input();
        duplicate_items.items.push(duplicate_items.items[0].clone());
        assert_eq!(
            repo.replace_current(duplicate_items),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn dangling_item_location_is_rejected_and_preserves_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut dangling = multi_input();
        dangling.items[0].location = ItemLocation::Character("missing-character".into());
        assert_eq!(
            repo.replace_current(dangling),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn present_character_location_is_accepted() {
        let mut repo = InMemoryInventoryRepository::new();
        repo.replace_current(input(1)).unwrap();

        let mut referenced = multi_input();
        referenced.items[0].location = ItemLocation::Character("character-2".into());
        let committed = repo.replace_current(referenced).unwrap();

        assert_eq!(committed.metadata.revision.value(), 2);
        assert_eq!(
            committed.items[0].location,
            ItemLocation::Character("character-2".into())
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
        let acquisition = ManifestAcquisition {
            version: DefinitionVersion::new("v1"),
            definitions: Vec::new(),
        };
        manifest.queue(Ok(acquisition.clone()));
        manifest.queue(Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(manifest.load_definitions().unwrap(), acquisition);
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
            .replace_definitions(replacement(
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
        assert_eq!(
            repository
                .current_metadata()
                .unwrap()
                .unwrap()
                .revision
                .value(),
            1
        );

        repository
            .replace_definitions(replacement(
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
        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
        assert_eq!(metadata.source, SourceMarker::Manifest);
        assert_eq!(metadata.checked_at, Timestamp::new(200));
        assert_eq!(metadata.revision.value(), 2);
    }

    #[test]
    fn definition_publication_is_idempotent_for_an_equal_normalized_set() {
        let mut repository = InMemoryDefinitionRepository::default();
        let input = replacement(
            "manifest-v1",
            vec![
                item_definition(7, "Alpha", "manifest-v1"),
                item_definition(8, "Beta", "manifest-v1"),
            ],
        );

        let first = repository.replace_definitions(input.clone()).unwrap();
        let version = repository.current_definition_version().unwrap();
        let alpha = repository.lookup_definition(ItemHash::new(7)).unwrap();
        let beta = repository.lookup_definition(ItemHash::new(8)).unwrap();

        let second = repository.replace_definitions(input).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.revision.value(), 1);
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
    fn definition_publication_advances_on_normalized_content_change() {
        let base = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let mut content_changed = base.clone();
        content_changed.definitions.definitions[0].name = "Alpha Changed".into();

        let mut version_changed = base.clone();
        version_changed.definitions = definition_set(
            "manifest-v2",
            vec![item_definition(7, "Alpha", "manifest-v2")],
        );

        for (label, changed) in [("content", content_changed), ("version", version_changed)] {
            let mut repository = InMemoryDefinitionRepository::default();
            repository.replace_definitions(base.clone()).unwrap();

            let metadata = repository.replace_definitions(changed).unwrap();

            assert_eq!(
                metadata.revision.value(),
                2,
                "changing {label} must advance"
            );
            assert_eq!(repository.publication_attempts(), 2);
        }
    }

    #[test]
    fn definition_revision_ignores_definition_order_but_not_tag_order() {
        let mut alpha = item_definition(7, "Alpha", "manifest-v1");
        alpha.tags = vec!["first".to_owned(), "second".to_owned()];
        let beta = item_definition(8, "Beta", "manifest-v1");

        let mut repository = InMemoryDefinitionRepository::default();
        let base = replacement("manifest-v1", vec![alpha.clone(), beta.clone()]);
        let first = repository.replace_definitions(base).unwrap();
        assert_eq!(first.revision.value(), 1);

        // A pure permutation of the complete set retains the revision: the port
        // publishes a set and the durable schema stores no definition ordinal,
        // so definition order is not content identity.
        let permuted = replacement("manifest-v1", vec![beta.clone(), alpha.clone()]);
        let retained = repository.replace_definitions(permuted).unwrap();
        assert_eq!(retained.revision.value(), 1);

        // Each record is compared in full, so tag order inside a record remains
        // identity and advancing the revision exactly once.
        let mut retagged = alpha;
        retagged.tags = vec!["second".to_owned(), "first".to_owned()];
        let advanced = repository
            .replace_definitions(replacement("manifest-v1", vec![retagged, beta]))
            .unwrap();
        assert_eq!(advanced.revision.value(), 2);
    }

    #[test]
    fn definition_publication_retains_revision_and_refreshes_checked_at() {
        let mut repository = InMemoryDefinitionRepository::default();
        let base = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let first = repository.replace_definitions(base.clone()).unwrap();
        assert_eq!(first.revision.value(), 1);
        assert_eq!(first.checked_at, Timestamp::new(200));

        let mut refreshed = base;
        refreshed.checked_at = Timestamp::new(9_000);
        let second = repository.replace_definitions(refreshed).unwrap();

        assert_eq!(second.revision.value(), 1);
        assert_eq!(second.checked_at, Timestamp::new(9_000));
        assert_eq!(second.source, SourceMarker::Manifest);
        assert_eq!(repository.current_metadata().unwrap(), Some(second));
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_rejects_invalid_source_and_preserves_metadata() {
        let mut repository = InMemoryDefinitionRepository::default();
        let committed = repository
            .replace_definitions(replacement(
                "manifest-v1",
                vec![item_definition(7, "Alpha", "manifest-v1")],
            ))
            .unwrap();

        let mut mismatched = replacement(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        );
        mismatched.source = SourceMarker::Profile;

        assert_eq!(
            repository.replace_definitions(mismatched),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repository.current_metadata().unwrap(), Some(committed));
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_overflow_is_atomic() {
        let original = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let mut repository = InMemoryDefinitionRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );

        // An unchanged refresh at the boundary retains the revision and refreshes
        // the committed timestamp rather than overflowing.
        let mut refreshed = original.clone();
        refreshed.checked_at = Timestamp::new(9_000);
        let retained = repository.replace_definitions(refreshed).unwrap();
        assert_eq!(retained.revision.value(), u64::MAX);
        assert_eq!(retained.checked_at, Timestamp::new(9_000));

        let changed = replacement(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        );
        assert_eq!(
            repository.replace_definitions(changed),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );

        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.revision.value(), u64::MAX);
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(repository.publication_attempts(), 2);

        let retained = repository.replace_definitions(original).unwrap();
        assert_eq!(retained.revision.value(), u64::MAX);
        assert_eq!(repository.publication_attempts(), 3);
    }

    #[test]
    fn definition_publication_rejects_duplicate_hashes_before_mutation() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let error = repository.replace_definitions(replacement(
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
            replacement(
                "manifest-v2",
                vec![
                    item_definition(7, "Alpha", "manifest-v2"),
                    item_definition(8, "Beta", "manifest-v1"),
                ],
            ),
            replacement("manifest-v2", Vec::new()),
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
        let seeded = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let mut repository =
            InMemoryDefinitionRepository::with_revision_for_test(seeded, SyncRevision::new(3))
                .with_publication_failure(AppError::new(ErrorCategory::ProviderUnavailable));

        let error = repository.replace_definitions(replacement(
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
        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.revision.value(), 3);
        assert_eq!(metadata.checked_at, Timestamp::new(200));
        assert_eq!(metadata.source, SourceMarker::Manifest);
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
