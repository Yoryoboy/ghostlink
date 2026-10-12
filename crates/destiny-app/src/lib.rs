//! Application-facing configuration, services, and sanitized diagnostics.

pub use destiny_core::armor_catalog::ArmorCatalogRepository;
pub use destiny_core::armor_model::{
    ARMOR_STATS, ArmorModel, ArmorSet, ArmorStat, ArmorStats, ArmorTuning, FragmentCapacity,
};
pub mod armor_model;
pub mod build_apply_exec;
pub mod builds;
pub use destiny_core::ports::{BuildCodec, BuildRepository};
pub use destiny_core::{
    armor_catalog, build, build_analysis, build_apply, build_mod_apply, build_sets, build_sockets,
    mod_assignment,
};
mod build_mods;
mod build_socket_evidence;
mod build_stats;
pub mod clarity;
pub use clarity::{ClaritySyncReport, ManifestAndClarityReport};
pub use destiny_core::{ClarityInsight, ClarityRepository};
pub mod equip_items;
pub use destiny_core::{
    EquipItemResult, EquipItemStatus, EquipItemsGateway, EquipItemsOutcome, EquipItemsRequest,
};
pub use equip_items::*;
pub mod apply_loadout;
pub mod loadouts;
pub use apply_loadout::*;
pub use destiny_core::{CharacterLoadouts, Loadout, LoadoutGateway, LoadoutItem, LoadoutSnapshot};
pub use destiny_core::{LoadoutActionGateway, LoadoutActionOutcome, LoadoutActionRejection};
pub use loadouts::{
    CharacterLoadoutsView, LoadoutItemView, LoadoutOperations, LoadoutPlugView, LoadoutService,
    LoadoutView, LoadoutsResult,
};
pub mod agents;
pub mod artifact;
pub use artifact::{
    ArtifactOperations, ArtifactPoints, ArtifactResult, ArtifactService, ArtifactSlot,
    CharacterArtifact, EquippedArtifact,
};
pub use destiny_core::{ArtifactGateway, ArtifactSnapshot};
pub mod auth;
pub mod auth_ux;
mod config;
mod defaults;
pub mod instanced_pull;
pub mod stack_pull;
pub use destiny_core::{
    StackBucketCapacityEvidence, StackBucketCapacityPolicy, StackPullGateway, StackPullOutcome,
    StackPullRefusal, StackPullRejection, StackPullRequest,
};
pub use instanced_pull::{
    InstancedPullExecutionFailure, InstancedPullExecutionFailureReason,
    InstancedPullExecutionOutcome, InstancedPullPlan, InstancedPullPlanRequest,
    InstancedPullPlanningError, InstancedPullRefusal, InstancedPullService, PreDispatchError,
    PullAttempt, PullContext, PullDisposition,
};
pub use stack_pull::*;
pub mod lock;
pub mod manifest;
pub mod mutation;
pub use destiny_core::{
    InsertPlugGateway, InsertPlugOutcome, InsertPlugRejection, InsertPlugRequest,
};
pub use destiny_core::{LockGateway, LockOutcome, LockRejection, LockRequest, SetLockStateRequest};
pub use lock::{
    LockExecutionEvidence, LockExecutionFailure, LockExecutionOutcome, LockPlan,
    LockVerificationFailure, LockVerificationFailureReason,
};
pub mod queries;
pub mod session;
pub mod setup;
pub mod sync;
pub use sync::{ProfileSyncWithObservationsOutcome, PublishedProfileAcquisition};
pub mod uri_handler;
pub mod weekly_personal;
pub mod weekly_projection;
pub mod weekly_selection;
pub use destiny_core::{
    AvailableActivity, ChallengeProgress, CharacterWeeklySnapshot, WeeklyProfileGateway,
};
pub mod weekly_reset;
pub use destiny_core::calendar;
pub use destiny_core::{ProfileInventoryRead, ProfileMintedAt};

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use auth::{AuthState, AuthorizationOutcome, AuthorizationService};
pub use config::paths::credentials_file;
pub use config::{
    ClientAuthMode, LoginCallbackUri, LoginRuntimeConfig, MAX_READ_FRESHNESS_TTL_SECONDS,
    ReadApiKey, ReadRuntimeConfig, RefreshClientConfig,
};
// Additive provider-neutral re-exports for presentation crates.
//
// `destiny-mcp` must translate MCP inputs and outputs against the shared read
// and mutation contracts and compose its read and mutation services without
// naming `destiny-core` in its manifest, and `destiny-cli` must dispatch reads
// against the same shared contracts. The public types that already appear in
// `QueryService` and `MutationService` requests, results, error categories,
// bounds, injected policy, and provider ports therefore have to be reachable
// through this crate. This block is the smallest additive surface that lets the
// presentation layer implement that translation and lets its binary composition
// root load a credential; it adds no behavior and no new abstraction.
pub use destiny_core::ArmorDefinition;
pub use destiny_core::InstancedPullRequest;
pub use destiny_core::domain;
pub use destiny_core::domain::ItemSlot;
pub use destiny_core::name_matching;
pub use destiny_core::ports::{
    DefinitionCatalogRepository, ManifestIndex, WorldDefinitionRepository,
};
pub use destiny_core::ports::{RetryDelay, StatDefinitionRepository};
pub use destiny_core::query::normalize_search_text;
pub use destiny_core::query::{InspectionPlug, InspectionPlugRole, LocationFilter, MAX_NAME_TERMS};
pub use destiny_core::search_query;
pub use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, BucketHash, Character, CharacterId,
    CharacterListResult, Clock, CredentialMaterial, CredentialSource, CredentialStore, Dataset,
    DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery, DefinitionVersion,
    DestinationSelector, DestinyMembership, ErrorCategory, ExecutionEvidence, FreshnessPolicy,
    FreshnessStatus, InspectItemRequest, InstanceSearchQuery, InventoryEffect, InventoryRepository,
    InventorySearchHit, InventorySearchResult, InventorySnapshot, InventorySnapshotInput,
    ItemDefinition, ItemEnergy, ItemHash, ItemHashFilter, ItemInspection, ItemInstance,
    ItemInstanceId, ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT,
    MAX_SEARCH_OFFSET, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS, MembershipId, MembershipType,
    MoveEffects, MoveIntentAuthorization, MovePlan, MoveRequest, PerkHash, PerkSummary, PlanId,
    PlanIdGenerator, PlanLifetime, ProfileAcquisition, ProfileGateway, ProfileSyncOutcome,
    ProfileSyncReason, QueryMetadata, RefreshGrant, RefreshToken, Resolution, SearchRepository,
    SearchTerm, SourceMarker, StaticCredentialSource, StepAction, StepOutcome, SyncMetadata,
    SyncRevision, Timestamp, TokenRefresh, TransferGateway, TransferOutcome, TransferRejection,
    TransferRequest,
};
pub use destiny_core::{
    ActivityDefinition, ActivityModifierDefinition, ActivityTypeDefinition, MilestoneDefinition,
    ObjectiveDefinition, PublicMilestone, PublicMilestoneActivity, PublicMilestonesGateway,
    PublicMilestonesSnapshot, StatDefinition, WorldDefinitionTable, WorldDefinitionsAcquisition,
};
pub use destiny_core::{
    CapacityRefusal, EquipmentCapacityEvidence, EquipmentCapacityPolicy,
    InstancedBucketObservation, InstancedPullGateway, InstancedPullOutcome, InstancedPullRejection,
};
pub use destiny_core::{
    CharacterPostmaster, PostmasterInstance, PostmasterResult, PostmasterStack,
    ProfileCurrenciesCoverage, StackContainer, StackObservation,
};
pub use destiny_core::{
    CharacterSubclass, CharacterSubclassOptions, ElementOptions, OptionGroup, OptionStatus,
    SubclassConfig, SubclassElement, SubclassOption, SubclassOptionsResult, SubclassPlugKind,
    SubclassResult, SubclassSlot,
};
pub use destiny_core::{
    EquipGateway, EquipItemRequest, EquipOutcome, EquipPlan, EquipPreflight, EquipRejection,
    EquipRequest,
};
pub use destiny_core::{MAX_STAT_CONDITIONS, NamedStat, StatCondition, StatOp};
pub use destiny_core::{ManifestAcquisition, ManifestDefinitions, ManifestGateway};
pub use manifest::{ManifestService, ManifestSyncReport, normalize_manifest};
pub use mutation::{
    ExecutionAuthorization, ExecutionFailure, IncompleteExecution, MoveExecutionOutcome,
    MutationError, MutationService, VerificationFailure, VerificationFailureReason,
};
pub use queries::{QueryOperations, QueryService, SearchInventoryRequest};
pub use session::{ConfiguredCredentialSource, RefreshingCredentialSource};
pub use sync::ProfileSyncService;
pub use weekly_personal::{
    ModifierCategory, PersonalActivity, PersonalChallenge, PersonalGroup, PersonalMilestone,
    PersonalObjective, PersonalReward, PhaseProgress, QuickplayPool, UnclassifiedActivity,
    UnclassifiedReason, UnclassifiedSource, VanguardAlerts, WeeklyCharacter, WeeklyPersonal,
    WeeklyPersonalService, WeeklyRotators, modifier_category,
};
pub use weekly_reset::{
    DefinitionResolution, WeeklyActivity, WeeklyMilestone, WeeklyModifier, WeeklyReset,
    WeeklyResetService,
};

// Additive provider-neutral re-exports for the interactive login composition.
//
// `destiny-cli` composes the authorization service from the shared
// ports and the provider adapters, and it must do so without naming
// `destiny-core` in its manifest. The authorization values, outcomes, and ports
// that the shared service and its adapters exchange therefore have to be
// reachable through this crate. This block adds no behavior and no new
// abstraction; it only makes the existing composition vocabulary public.
pub use destiny_core::{
    AuthorizationCallback, AuthorizationCode, AuthorizationRequest, AuthorizationState,
    BrowserAuthorization, CallbackReceiver, CallbackResult, PublicTokenExchange, SecureRandom,
};

/// Reports that the application foundation is available.
pub fn foundation_ready() -> bool {
    destiny_core::foundation_ready()
}

/// Loads and validates required runtime configuration for application internals.
///
/// This remains crate-visible so future application services can consume the
/// validated configuration without exposing provider credentials to adapters.
pub(crate) fn load_config() -> Result<config::Config, AppError> {
    config::Config::from_env()
}

/// Validates startup configuration and returns only a sanitized application
/// error on failure.
pub fn startup() -> Result<(), AppError> {
    load_config().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{AppError, startup};
    use crate::config::Config;
    use std::collections::HashMap;

    #[test]
    fn startup_diagnostic_does_not_echo_supplied_configuration_values() {
        let values = HashMap::from([
            ("BUNGIE_API_KEY".into(), "secret-api-key".into()),
            ("BUNGIE_PUBLIC_CLIENT_ID".into(), "secret-client-id".into()),
            (
                "BUNGIE_CALLBACK_URI".into(),
                "http://example.test:43127/callback?code=secret".into(),
            ),
            (
                "BUNGIE_REGISTERED_CALLBACK_URI".into(),
                "http://example.test:43127/callback?code=secret".into(),
            ),
        ]);
        let error = match Config::from_values(&values) {
            Ok(_) => panic!("invalid configuration should fail"),
            Err(error) => error,
        };

        let diagnostic = error.to_string();
        assert_eq!(
            diagnostic,
            "required runtime configuration is missing or invalid"
        );
        for supplied in [
            "secret-api-key",
            "secret-client-id",
            "example.test",
            "secret",
        ] {
            assert!(
                !diagnostic.contains(supplied),
                "diagnostic leaked {supplied}"
            );
        }
    }

    #[test]
    fn startup_exposes_only_the_sanitized_result_type() {
        let startup_fn: fn() -> Result<(), AppError> = startup;
        let _ = startup_fn;
    }

    #[test]
    fn presentation_crates_do_not_depend_directly_on_destiny_core() {
        for manifest in [
            concat!(env!("CARGO_MANIFEST_DIR"), "/../destiny-cli/Cargo.toml"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../destiny-mcp/Cargo.toml"),
        ] {
            let contents = std::fs::read_to_string(manifest).expect("workspace manifest");
            assert!(
                !contents
                    .lines()
                    .any(|line| { line.trim_start().starts_with("destiny-core") }),
                "presentation crate directly depends on destiny-core: {manifest}"
            );
        }
    }
}
