//! Minimal provider-neutral behavioral ports.

use crate::domain::{Character, InventorySnapshot, ItemDefinition, ItemInstance};
use crate::error::AppError;
use crate::ids::{DefinitionVersion, ItemHash, MembershipId, MembershipType, PlanId};
use crate::state::{Resolution, Timestamp};
use std::fmt;

/// Supplies application time without coupling the core to a clock runtime.
pub trait Clock {
    fn now(&self) -> Timestamp;
}

/// Supplies opaque plan identifiers without prescribing an ID algorithm.
pub trait PlanIdGenerator {
    fn next_plan_id(&mut self) -> PlanId;
}

/// A normalized inventory value before repository-owned revision assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySnapshotInput {
    pub membership_id: MembershipId,
    pub membership_type: MembershipType,
    pub definition_version: DefinitionVersion,
    pub characters: Vec<Character>,
    pub items: Vec<ItemInstance>,
}

impl InventorySnapshotInput {
    pub fn with_metadata(snapshot: &InventorySnapshot) -> Self {
        Self {
            membership_id: snapshot.metadata.membership_id.clone(),
            membership_type: snapshot.metadata.membership_type,
            definition_version: snapshot.metadata.definition_version.clone(),
            characters: snapshot.characters.clone(),
            items: snapshot.items.clone(),
        }
    }
}

/// Reads the active normalized definitions.
pub trait DefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError>;
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError>;
}

/// Reads and atomically replaces the current normalized inventory projection.
pub trait InventoryRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError>;
    /// The repository assigns or retains the revision and publishes only on success.
    fn replace_current(
        &mut self,
        snapshot: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError>;
}

/// Returns normalized account state; transport and payload types stop at the adapter.
pub trait ProfileGateway {
    fn load_inventory(&mut self) -> Result<InventorySnapshotInput, AppError>;
}

/// Versioned normalized definitions returned by a manifest boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDefinitions {
    pub version: DefinitionVersion,
    pub definitions: Vec<ItemDefinition>,
}

/// Returns normalized definitions, not locations, URLs, or serialized payloads.
pub trait ManifestGateway {
    fn load_definitions(&mut self) -> Result<ManifestDefinitions, AppError>;
}

/// Opaque credential material. Its bytes are intentionally unavailable to formatting APIs.
#[derive(Clone, Eq, PartialEq)]
pub struct CredentialMaterial(Vec<u8>);

impl CredentialMaterial {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for CredentialMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialMaterial")
    }
}

/// Stores one opaque credential value without prescribing persistence or serialization.
pub trait CredentialStore {
    fn save(&mut self, material: CredentialMaterial) -> Result<(), AppError>;
    fn load(&self) -> Result<Option<CredentialMaterial>, AppError>;
    fn delete(&mut self) -> Result<(), AppError>;
}
