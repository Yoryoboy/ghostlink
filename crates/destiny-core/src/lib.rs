//! Provider- and transport-free domain contracts.

pub use clarity::*;
pub mod calendar;
pub mod clarity;
pub use calendar::*;
pub mod loadout;
pub use loadout::*;
pub mod loadout_action;
pub use loadout_action::*;
pub mod agents;
pub mod artifact;
pub use artifact::*;
pub mod auth;
pub mod domain;
pub mod equip_items;
pub use equip_items::*;
pub mod equipment_capacity;
pub mod error;
pub use equipment_capacity::{
    CapacityPolicyMismatch, CapacityRefusal, EquipmentCapacityEvidence, EquipmentCapacityPolicy,
    EquipmentCapacityProof, LocalCapacityError, LocalEquipmentOccupancy,
    QualifiedEquipmentCapacityPolicy,
};
pub mod ids;
pub mod instanced_pull;
pub use instanced_pull::{InstancedPullOutcome, InstancedPullRejection, InstancedPullRequest};
pub mod inventory_effects;
pub use inventory_effects::{
    InventoryEffect, InventoryEffectError, InventoryEffectsResult, apply_inventory_effects,
};
pub mod mutation;
pub mod provider_rejection;
pub use provider_rejection::ProviderRejection;
pub mod name_matching;
pub mod ports;
pub mod postmaster;
pub mod profile_observations;
pub use postmaster::{CharacterPostmaster, PostmasterInstance, PostmasterResult, PostmasterStack};
pub use profile_observations::{
    InstancedBucketObservation, ProfileAcquisition, ProfileInventoryRead,
};
pub mod public_milestones;
pub mod query;
pub use ports::PublicMilestonesGateway;
pub use public_milestones::{PublicMilestone, PublicMilestoneActivity, PublicMilestonesSnapshot};
pub mod redact;
pub mod stack_bucket_capacity;
pub use stack_bucket_capacity::{
    QualifiedStackBucketCapacityPolicy, StackBucketCapacityEvidence, StackBucketCapacityPolicy,
};
pub mod stack_pull;
pub use ports::StackPullGateway;
pub use stack_pull::{
    StackPullCapacityInput, StackPullDecision, StackPullOutcome, StackPullRefusal,
    StackPullRejection, StackPullRequest, evaluate_stack_pull, evaluate_stack_pull_capacity,
};
pub mod stacks;
pub use stacks::{ProfileCurrenciesCoverage, StackContainer, StackObservation};
pub mod state;
pub mod uri_handler;
pub mod weekly_profile;
pub use weekly_profile::*;
pub mod world_definitions;
pub use world_definitions::{
    ActivityDefinition, ActivityModifierDefinition, ActivityTypeDefinition, ItemDescription,
    LoadoutNameDefinition, MilestoneDefinition, ObjectiveDefinition, StatDefinition,
    WorldDefinitionTable, WorldDefinitionsAcquisition, normalize_stat_name,
};

pub use auth::{
    AccessCredential, AccessCredentialMetadata, AuthorizationCallback, AuthorizationCode,
    AuthorizationState, CallbackResult, ClientSecret, RefreshGrant, RefreshToken,
};
pub use domain::{
    AvailablePlug, Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation,
    PerkSummary, PlugSetEntry, PlugSetScope, PlugSummary, SnapshotMetadata, StatSummary,
    SubclassElement, SubclassPlugKind, SyncMetadata, classify_subclass_plug,
    parse_subclass_category,
};
pub use error::{AppError, ErrorCategory};
pub use ids::{
    BucketHash, CharacterId, CorrelationId, DefinitionVersion, ItemHash, ItemInstanceId,
    MembershipId, MembershipType, PerkHash, PlanId, SyncRevision,
};
pub use mutation::{
    DestinationSelector, EquipPlan, EquipPreflight, EquipRequest, ExecutionEvidence,
    ExecutionTicket, IntentAuthorizationOutcome, ItemSelector, LockRequest, MAX_MOVE_STEPS,
    MAX_MUTATION_STEPS, MAX_PLAN_LIFETIME_SECONDS, MoveEffects, MoveIntent,
    MoveIntentAuthorization, MovePlan, MovePlanValidation, MoveRequest, MutationPolicyInput,
    MutationPolicyOutcome, MutationStep, PlanBinding, PlanLifetime, PlanLifetimeValidation,
    StepAction, StepOutcome, StepRecord, character_destination, destination_matches,
    evaluate_mutation, plan_steps, resolve_destination,
};
pub use ports::{
    AuthorizationRequest, BrowserAuthorization, CallbackReceiver, Clock, CredentialMaterial,
    CredentialSource, CredentialStore, DefinitionReplacementInput, DefinitionRepository,
    DestinyMembership, EquipGateway, EquipItemRequest, EquipOutcome, EquipRejection,
    InstancedPullGateway, InventoryRepository, InventorySnapshotInput, LockGateway, LockOutcome,
    LockRejection, ManifestAcquisition, ManifestDefinitionCandidate, ManifestDefinitions,
    ManifestGateway, PlanIdGenerator, ProfileGateway, PublicTokenExchange, SearchRepository,
    SecureRandom, SetLockStateRequest, StatDefinitionRepository, StaticCredentialSource,
    TokenRefresh, TransferGateway, TransferOutcome, TransferRejection, TransferRequest,
};
pub use query::{
    ARTIFACT_BUCKET_HASH, CharacterListResult, CharacterSubclass, CharacterSubclassOptions,
    DefinitionSearchQuery, ElementOptions, InspectItemRequest, InstanceSearchQuery,
    InventorySearchHit, InventorySearchResult, ItemHashFilter, ItemInspection, LocationFilter,
    MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_OFFSET, MAX_SEARCH_TEXT_LENGTH,
    MAX_STAT_CONDITIONS, MAX_TAG_FILTERS, NamedStat, OptionGroup, OptionStatus, QueryMetadata,
    SUBCLASS_BUCKET_HASH, SearchTerm, StatCondition, StatOp, SubclassConfig, SubclassOption,
    SubclassOptionsResult, SubclassResult, SubclassSlot, definition_matches, instance_matches,
    normalize_search_text,
};
pub use query::{InspectionPlug, InspectionPlugRole};
pub use redact::{REDACTED, Redacted, opaque, redact_optional};
pub use state::{
    Dataset, FreshnessInput, FreshnessPolicy, FreshnessStatus, ProfileMintedAt, ProfileSyncOutcome,
    ProfileSyncReason, Resolution, RevisionAllocation, RevisionOverflow, RevisionState,
    SemanticResolution, SourceMarker, Timestamp, allocate_revision,
};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
