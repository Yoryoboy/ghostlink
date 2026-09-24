//! Application-facing configuration, services, and sanitized diagnostics.

pub mod auth;
pub mod auth_ux;
mod config;
pub mod manifest;
pub mod mutation;
pub mod queries;
pub mod sync;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use auth::{AuthState, AuthorizationOutcome, AuthorizationService};
pub use config::{MAX_READ_FRESHNESS_TTL_SECONDS, ReadApiKey, ReadRuntimeConfig};
pub use destiny_core::{AppError, ErrorCategory};

// Additive provider-neutral re-exports for presentation crates.
//
// `destiny-mcp` must translate MCP inputs and outputs against the shared read
// and mutation contracts and compose its read and mutation services without
// naming `destiny-core` in its manifest. The public types that already appear in
// `QueryService` and `MutationService` requests, results, error categories,
// bounds, injected policy, and provider ports therefore have to be reachable
// through this crate. This block is the smallest additive surface that lets the
// presentation layer implement that translation and lets its binary composition
// root load a credential; it adds no behavior and no new abstraction.
pub use destiny_core::{
    AccessCredential, AccessCredentialMetadata, Character, CharacterId, CharacterListResult, Clock,
    CredentialMaterial, CredentialStore, Dataset, DefinitionReplacementInput, DefinitionRepository,
    DefinitionSearchQuery, DefinitionVersion, DestinationSelector, DestinyMembership,
    ExecutionEvidence, FreshnessPolicy, FreshnessStatus, InspectItemRequest, InstanceSearchQuery,
    InventoryRepository, InventorySearchResult, InventorySnapshot, InventorySnapshotInput,
    ItemDefinition, ItemHash, ItemHashFilter, ItemInspection, ItemInstance, ItemInstanceId,
    ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH,
    MAX_TAG_FILTERS, MembershipId, MembershipType, MoveEffects, MoveIntentAuthorization, MovePlan,
    MoveRequest, PerkHash, PerkSummary, PlanId, PlanIdGenerator, PlanLifetime, ProfileGateway,
    ProfileSyncOutcome, ProfileSyncReason, QueryMetadata, Resolution, SearchRepository, SearchTerm,
    SourceMarker, StepOutcome, SyncMetadata, Timestamp, TransferGateway, TransferOutcome,
    TransferRejection, TransferRequest,
};
pub use manifest::{ManifestService, normalize_manifest};
pub use mutation::{
    ExecutionAuthorization, ExecutionFailure, IncompleteExecution, MoveExecutionOutcome,
    MutationError, MutationService, VerificationFailure, VerificationFailureReason,
};
pub use queries::{QueryService, SearchInventoryRequest};
pub use sync::ProfileSyncService;

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
