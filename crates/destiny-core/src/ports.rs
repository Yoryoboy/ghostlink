//! Minimal provider-neutral behavioral ports.

use crate::auth::{AccessCredential, AuthorizationCallback, AuthorizationCode, AuthorizationState};
use crate::domain::{Character, InventorySnapshot, ItemDefinition, ItemInstance};
use crate::error::AppError;
use crate::ids::{BucketHash, DefinitionVersion, ItemHash, MembershipId, MembershipType, PlanId};
use crate::state::{Resolution, Timestamp};
use std::fmt;

/// Supplies application time without coupling the core to a clock runtime.
pub trait Clock {
    fn now(&self) -> Result<Timestamp, AppError>;
}

/// Supplies opaque plan identifiers without prescribing an ID algorithm.
/// Generates unpredictable, opaque authorization state values.
pub trait SecureRandom {
    fn generate_state(&mut self) -> Result<AuthorizationState, AppError>;
}

/// A provider-neutral request to begin browser authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationRequest {
    pub state: AuthorizationState,
    pub expires_at: Timestamp,
}

pub trait BrowserAuthorization {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError>;
}

pub trait CallbackReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError>;
}

/// Exchanges a code for public-client access material; no refresh or secret API exists here.
pub trait PublicTokenExchange {
    fn exchange(&mut self, code: AuthorizationCode) -> Result<AccessCredential, AppError>;
}

pub trait PlanIdGenerator {
    fn next_plan_id(&mut self) -> Result<PlanId, AppError>;
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

/// Reads the active normalized definitions and publishes complete replacements.
///
/// A publishable [`ManifestDefinitions`] set is complete, internally consistent,
/// and revision-free. Publication replaces every previously visible definition,
/// rejects an empty set, duplicate hashes, and mixed definition versions before
/// mutating state, and leaves the prior set readable when it fails. Durable
/// revision allocation remains WU-005 scope.
pub trait DefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError>;
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError>;
    /// Atomically publishes one complete normalized set, removing old definitions.
    fn replace_definitions(&mut self, definitions: ManifestDefinitions) -> Result<(), AppError>;
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

/// A complete versioned normalized definition set published as one replacement.
///
/// It carries no durable revision: normalized equality is deterministic, and
/// revision allocation remains WU-005 scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDefinitions {
    pub version: DefinitionVersion,
    pub definitions: Vec<ItemDefinition>,
}

/// One untrusted, transport-neutral Manifest definition candidate.
///
/// An adapter maps an upstream payload into this provider-neutral shape, and the
/// application Manifest service alone decides whether the values are bounded,
/// complete, and internally consistent. The record mirrors the normalized
/// [`ItemDefinition`] fields and is identified only by a definition
/// [`ItemHash`]; definition and instance identity are separated by the type
/// boundary, because a runtime instance identifier has no place in a Manifest
/// record and needs no provider metadata to rule out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDefinitionCandidate {
    pub hash: ItemHash,
    pub bucket_hash: Option<BucketHash>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    pub element: Option<String>,
    pub tags: Vec<String>,
    pub version: DefinitionVersion,
}

/// One versioned batch of untrusted definition candidates from a Manifest gateway.
///
/// Neither the batch version nor its candidates are trusted: they may be blank,
/// oversized, inconsistent, duplicated, or incomplete. The application Manifest
/// service validates and normalizes the batch before any repository mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestAcquisition {
    pub version: DefinitionVersion,
    pub definitions: Vec<ManifestDefinitionCandidate>,
}

/// Returns untrusted normalized-shape candidates, not locations, URLs, or
/// serialized payloads.
pub trait ManifestGateway {
    fn load_definitions(&mut self) -> Result<ManifestAcquisition, AppError>;
}

/// Opaque credential material. Bytes are available only through the explicit
/// trusted-adapter exposure boundary and are never available to formatting APIs.
#[derive(Clone, Eq, PartialEq)]
pub struct CredentialMaterial(Vec<u8>);

impl CredentialMaterial {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// Exposes secret bytes to a trusted credential-store/provider adapter.
    pub fn expose_secret(&self) -> &[u8] {
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
