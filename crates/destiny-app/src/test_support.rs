//! Offline, provider-neutral fakes for application contract tests.
//!
//! These types are test support only. They model normalized behavior and never
//! perform network, filesystem, serialization, or credential-provider work.

use destiny_core::{
    AppError, Clock, CredentialMaterial, CredentialStore, DefinitionRepository, DefinitionVersion,
    InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemDefinition, ItemHash,
    ManifestDefinitions, ManifestGateway, PlanId, PlanIdGenerator, ProfileGateway, Resolution,
    SnapshotMetadata, SyncRevision, Timestamp,
};
use std::collections::VecDeque;

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
    pub const fn advance(&mut self, seconds: u64) {
        self.now = Timestamp::new(self.now.seconds() + seconds);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        self.now
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
    fn next_plan_id(&mut self) -> PlanId {
        let id = PlanId::new(format!("plan-{}", self.next));
        self.next += 1;
        id
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryDefinitionRepository {
    version: Option<DefinitionVersion>,
    definitions: Vec<ItemDefinition>,
}

impl InMemoryDefinitionRepository {
    pub fn new(version: DefinitionVersion, definitions: Vec<ItemDefinition>) -> Self {
        Self {
            version: Some(version),
            definitions,
        }
    }
}

impl DefinitionRepository for InMemoryDefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.version
            .clone()
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::LocalStorage))
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
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryInventoryRepository {
    current: Option<InventorySnapshot>,
    fingerprint: Option<u64>,
}

impl InMemoryInventoryRepository {
    pub fn new() -> Self {
        Self::default()
    }

    fn fingerprint(input: &InventorySnapshotInput) -> u64 {
        let mut value = 0xcbf29ce484222325u64;
        fn add(value: &mut u64, bytes: &[u8]) {
            for byte in bytes {
                *value = value
                    .wrapping_mul(0x100000001b3)
                    .wrapping_add(u64::from(*byte));
            }
        }
        add(&mut value, input.membership_id.as_str().as_bytes());
        add(&mut value, &input.membership_type.value().to_le_bytes());
        add(&mut value, input.definition_version.as_str().as_bytes());
        for character in &input.characters {
            add(&mut value, character.id.as_str().as_bytes());
            add(
                &mut value,
                character.name.as_deref().unwrap_or("").as_bytes(),
            );
        }
        for item in &input.items {
            add(&mut value, item.instance_id.as_str().as_bytes());
            add(&mut value, &item.item_hash.value().to_le_bytes());
            add(&mut value, &item.quantity.to_le_bytes());
            add(&mut value, &[item.equipped as u8, item.locked as u8]);
        }
        value
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
        let fingerprint = Self::fingerprint(&input);
        let revision = match (self.current.as_ref(), self.fingerprint) {
            (Some(current), Some(previous)) if previous == fingerprint => current.metadata.revision,
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
        self.fingerprint = Some(fingerprint);
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

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        Character, ErrorCategory, ItemInstance, ItemLocation, MembershipId, MembershipType,
    };

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

    #[test]
    fn clock_and_ids_are_deterministic() {
        let mut clock = FakeClock::fixed(Timestamp::new(10));
        assert_eq!(clock.now(), Timestamp::new(10));
        clock.advance(5);
        assert_eq!(clock.now(), Timestamp::new(15));
        let mut ids = DeterministicPlanIds::starting_at(4);
        assert_eq!(ids.next_plan_id().as_str(), "plan-4");
        assert_eq!(ids.next_plan_id().as_str(), "plan-5");
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
    fn credential_material_is_redacted_and_never_displayable() {
        let material = CredentialMaterial::from_bytes(b"secret");
        assert_eq!(format!("{material:?}"), "CredentialMaterial");
        let mut store = InMemoryCredentialStore::default();
        store.save(material).unwrap();
        assert!(store.contains_material());
        assert_eq!(store.load().unwrap().unwrap().as_bytes(), b"secret");
        store.delete().unwrap();
        assert!(!store.contains_material());
    }
}
