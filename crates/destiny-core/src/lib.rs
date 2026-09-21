//! Provider- and transport-free domain contracts.

pub mod domain;
pub mod error;
pub mod ids;

pub use domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, SnapshotMetadata,
};
pub use error::{AppError, ErrorCategory};
pub use ids::{
    BucketHash, CharacterId, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId,
    MembershipType, PlanId, SyncRevision,
};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
