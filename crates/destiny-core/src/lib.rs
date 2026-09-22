//! Provider- and transport-free domain contracts.

pub mod auth;
pub mod domain;
pub mod error;
pub mod ids;
pub mod mutation;
pub mod ports;
pub mod state;

pub use auth::{
    AccessCredential, AccessCredentialMetadata, AuthorizationCallback, AuthorizationCode,
    AuthorizationState, CallbackResult,
};
pub use domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, PerkSummary,
    SnapshotMetadata, SyncMetadata,
};
pub use error::{AppError, ErrorCategory};
pub use ids::{
    BucketHash, CharacterId, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId,
    MembershipType, PerkHash, PlanId, SyncRevision,
};
pub use mutation::{
    MoveIntent, MovePlan, MovePlanValidation, MutationPolicyInput, MutationPolicyOutcome,
    evaluate_mutation,
};
pub use ports::{
    AuthorizationRequest, BrowserAuthorization, CallbackReceiver, Clock, CredentialMaterial,
    CredentialStore, DefinitionReplacementInput, DefinitionRepository, DestinyMembership,
    InventoryRepository, InventorySnapshotInput, ManifestAcquisition, ManifestDefinitionCandidate,
    ManifestDefinitions, ManifestGateway, PlanIdGenerator, ProfileGateway, PublicTokenExchange,
    SecureRandom,
};
pub use state::{
    Dataset, FreshnessInput, FreshnessPolicy, FreshnessStatus, ProfileSyncOutcome,
    ProfileSyncReason, Resolution, RevisionAllocation, RevisionOverflow, RevisionState,
    SemanticResolution, SourceMarker, Timestamp, allocate_revision,
};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
