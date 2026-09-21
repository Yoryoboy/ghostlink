//! Provider- and transport-free domain contracts.

pub mod domain;
pub mod error;
pub mod ids;
pub mod mutation;
pub mod ports;
pub mod state;

pub use domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, SnapshotMetadata,
};
pub use error::{AppError, ErrorCategory};
pub use ids::{
    BucketHash, CharacterId, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId,
    MembershipType, PlanId, SyncRevision,
};
pub use mutation::{
    MoveIntent, MovePlan, MovePlanValidation, MutationPolicyInput, MutationPolicyOutcome,
    evaluate_mutation,
};
pub use ports::{
    Clock, CredentialMaterial, CredentialStore, DefinitionRepository, InventoryRepository,
    InventorySnapshotInput, ManifestDefinitions, ManifestGateway, PlanIdGenerator, ProfileGateway,
};
pub use state::{
    Dataset, FreshnessInput, FreshnessPolicy, FreshnessStatus, Resolution, RevisionAllocation,
    RevisionOverflow, RevisionState, SemanticResolution, SourceMarker, Timestamp,
    allocate_revision,
};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
