//! `destiny-mcp` process composition root.
//!
//! This binary is the single place in the workspace that names concrete
//! provider and platform adapters. It is deliberately small: it loads the
//! redacted shared read runtime configuration, loads the complete stored access
//! credential, builds the authenticated Bungie profile gateway and the SQLite
//! projection store, wraps them in the application query service, and serves the
//! read tools over stdio.
//!
//! # Architecture
//!
//! The library crate (`destiny_mcp`) holds the RMCP server, the bounded MCP
//! schemas, and the injectable [`InventoryReadService`] seam, and never names an
//! adapter. This binary is the composition root that satisfies the seam with
//! concrete adapters, which is why `destiny-adapters` is a dependency of this
//! crate rather than of the library contracts. The read path is:
//!
//! `stdio -> rmcp server -> Box<dyn InventoryReadService + Send> -> QueryReadService -> QueryService -> ProfileSyncService -> BungieProfileGateway / SqliteStore`.
//!
//! # Protocol discipline
//!
//! Stdout carries JSON-RPC frames only. Startup failures and every runtime
//! failure are sanitized [`AppError`] values printed to stderr; no credential,
//! provider payload, or configuration value can appear in either stream.

use clap::Parser;
use destiny_adapters::sqlite::SqliteStore;
use destiny_adapters::{
    BungieProfileGateway, HttpProfileTransport, NativeCredentialStore, ProfileApiKey, SystemClock,
};
use destiny_app::{
    AccessCredential, AppError, CredentialStore, ErrorCategory, ProfileSyncService, QueryService,
    ReadRuntimeConfig,
};
use destiny_mcp::runtime::serve_stdio;
use destiny_mcp::{InventoryReadService, QueryReadService, ReadToolServer};

#[derive(Debug, Parser)]
#[command(
    name = "destiny-mcp",
    version,
    about = "Destiny inventory MCP server over stdio"
)]
struct Cli;

#[tokio::main]
async fn main() {
    let _ = Cli::parse();
    if let Err(error) = run().await {
        // Startup and runtime diagnostics go to stderr; stdout belongs to the
        // protocol. `AppError` carries only a sanitized category message.
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Loads configuration, composes the read service, and serves it over stdio.
async fn run() -> Result<(), AppError> {
    let config = ReadRuntimeConfig::from_env()?;
    let store = NativeCredentialStore::new()?;
    let server = compose_read_server(&config, &store)?;
    serve_stdio(server).await
}

/// Builds the read tool server from the runtime configuration and a credential store.
///
/// The credential is loaded first, so a missing or unusable credential fails as
/// a sanitized authorization error before any transport, database, or network
/// work is attempted. This makes the composition root's failure paths testable
/// with an injected store and no live keyring, provider, or database.
fn compose_read_server<S: CredentialStore>(
    config: &ReadRuntimeConfig,
    store: &S,
) -> Result<ReadToolServer, AppError> {
    let credential = load_credential(store)?;
    let api_key = ProfileApiKey::new(config.api_key().expose_secret())?;
    let transport = HttpProfileTransport::new()?;
    let clock = SystemClock::new();
    let gateway = BungieProfileGateway::new(
        transport,
        clock,
        credential,
        api_key,
        config.definition_version().clone(),
    )?;
    let repository = SqliteStore::open(config.database_path())?;
    let sync = ProfileSyncService::new(gateway, repository);
    let queries = QueryService::new(sync);
    let read_service = QueryReadService::new(queries, clock, config.freshness_policy());
    let boxed: Box<dyn InventoryReadService + Send> = Box::new(read_service);
    Ok(ReadToolServer::new(boxed))
}

/// Loads the complete stored credential, failing closed when it is absent.
///
/// A missing entry is an explicit [`ErrorCategory::AuthorizationRequired`]
/// failure: the process never falls back to an environment value, a fixture, or
/// a plaintext file. A store error propagates unchanged, so a legacy
/// raw-material entry stays [`ErrorCategory::ReauthorizationRequired`] and a
/// corrupt or unavailable store stays [`ErrorCategory::CredentialStoreUnavailable`].
fn load_credential<S: CredentialStore>(store: &S) -> Result<AccessCredential, AppError> {
    match store.load()? {
        Some(credential) => Ok(credential),
        None => Err(AppError::new(ErrorCategory::AuthorizationRequired)),
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_read_server, load_credential};
    use destiny_app::test_support::{FailingCredentialStore, InMemoryCredentialStore};
    use destiny_app::{
        AccessCredential, AccessCredentialMetadata, CredentialMaterial, CredentialStore,
        ErrorCategory, ReadRuntimeConfig, Timestamp,
    };
    use std::collections::HashMap;

    const API_KEY_SENTINEL: &str = "api-key-sentinel";
    const CREDENTIAL_SENTINEL: &str = "credential-sentinel";

    fn config() -> ReadRuntimeConfig {
        let values = HashMap::from([
            ("BUNGIE_API_KEY".into(), API_KEY_SENTINEL.into()),
            (
                "DESTINY_DATABASE_PATH".into(),
                "/tmp/destiny-mcp-test.db".into(),
            ),
            ("DESTINY_DEFINITION_VERSION".into(), "manifest-v1".into()),
            ("DESTINY_FRESHNESS_TTL_SECONDS".into(), "900".into()),
        ]);
        ReadRuntimeConfig::from_values(&values).expect("valid configuration")
    }

    fn credential() -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_700_000_000),
            },
            material: CredentialMaterial::from_bytes(CREDENTIAL_SENTINEL.as_bytes().to_vec()),
        }
    }

    #[test]
    fn missing_credential_fails_closed_without_touching_the_environment() {
        let error = load_credential(&InMemoryCredentialStore::default())
            .expect_err("a missing credential fails closed");
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        assert_eq!(error.to_string(), "authorization is required");
        assert!(!format!("{error} {error:?}").contains(API_KEY_SENTINEL));
    }

    #[test]
    fn present_credential_is_returned_whole() {
        let mut store = InMemoryCredentialStore::default();
        let expected = credential();
        store
            .save(expected.clone())
            .expect("store accepts credential");
        assert_eq!(load_credential(&store).expect("credential loads"), expected);
    }

    #[test]
    fn legacy_credential_propagates_reauthorization_required() {
        let store = FailingCredentialStore {
            error: destiny_app::AppError::new(ErrorCategory::ReauthorizationRequired),
        };
        let error = load_credential(&store).expect_err("legacy entry fails closed");
        assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);
    }

    #[test]
    fn unavailable_store_propagates_its_sanitized_category() {
        let store = FailingCredentialStore {
            error: destiny_app::AppError::new(ErrorCategory::CredentialStoreUnavailable),
        };
        let error = load_credential(&store).expect_err("store failure propagates");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
    }

    #[test]
    fn composition_fails_before_any_adapter_work_when_the_credential_is_missing() {
        // The database path does not exist and the transport is never built,
        // because the missing credential fails first. This proves the ordering
        // of the composition root without a live keyring, network, or database.
        let error = compose_read_server(&config(), &InMemoryCredentialStore::default())
            .expect_err("missing credential stops composition");
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        assert!(!error.to_string().contains(API_KEY_SENTINEL));
    }

    #[test]
    fn composition_fails_before_any_adapter_work_for_a_legacy_credential() {
        let store = FailingCredentialStore {
            error: destiny_app::AppError::new(ErrorCategory::ReauthorizationRequired),
        };
        let error = compose_read_server(&config(), &store)
            .expect_err("legacy credential stops composition");
        assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);
    }
}
