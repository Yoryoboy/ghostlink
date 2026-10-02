//! Provider- and transport-free domain contracts.

pub mod auth;
pub mod domain;
pub mod error;
pub mod ids;
pub mod mutation;
pub mod ports;
pub mod query;
pub mod redact;
pub mod state;

pub use auth::{
    AccessCredential, AccessCredentialMetadata, AuthorizationCallback, AuthorizationCode,
    AuthorizationState, CallbackResult, RefreshGrant, RefreshToken,
};
pub use domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, PerkSummary,
    SnapshotMetadata, SyncMetadata,
};
pub use error::{AppError, ErrorCategory};
pub use ids::{
    BucketHash, CharacterId, CorrelationId, DefinitionVersion, ItemHash, ItemInstanceId,
    MembershipId, MembershipType, PerkHash, PlanId, SyncRevision,
};
pub use mutation::{
    DestinationSelector, ExecutionEvidence, ExecutionTicket, IntentAuthorizationOutcome,
    ItemSelector, MAX_MOVE_STEPS, MAX_PLAN_LIFETIME_SECONDS, MoveEffects, MoveIntent,
    MoveIntentAuthorization, MovePlan, MovePlanValidation, MoveRequest, MoveStep,
    MutationPolicyInput, MutationPolicyOutcome, PlanBinding, PlanLifetime, PlanLifetimeValidation,
    StepOutcome, StepRecord, character_destination, destination_matches, evaluate_mutation,
    plan_steps, resolve_destination,
};
pub use ports::{
    AuthorizationRequest, BrowserAuthorization, CallbackReceiver, Clock, CredentialMaterial,
    CredentialSource, CredentialStore, DefinitionReplacementInput, DefinitionRepository,
    DestinyMembership, InventoryRepository, InventorySnapshotInput, ManifestAcquisition,
    ManifestDefinitionCandidate, ManifestDefinitions, ManifestGateway, PlanIdGenerator,
    ProfileGateway, PublicTokenExchange, SearchRepository, SecureRandom, StaticCredentialSource,
    TokenRefresh, TransferGateway, TransferOutcome, TransferRejection, TransferRequest,
};
pub use query::{
    CharacterListResult, DefinitionSearchQuery, InspectItemRequest, InstanceSearchQuery,
    InventorySearchHit, InventorySearchResult, ItemHashFilter, ItemInspection, MAX_HASH_FILTERS,
    MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS, QueryMetadata, SearchTerm,
    definition_matches, instance_matches, normalize_search_text,
};
pub use redact::{REDACTED, Redacted, opaque, redact_optional};
pub use state::{
    Dataset, FreshnessInput, FreshnessPolicy, FreshnessStatus, ProfileSyncOutcome,
    ProfileSyncReason, Resolution, RevisionAllocation, RevisionOverflow, RevisionState,
    SemanticResolution, SourceMarker, Timestamp, allocate_revision,
};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
