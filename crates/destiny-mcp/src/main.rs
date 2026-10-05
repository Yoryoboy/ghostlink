//! `destiny-mcp` process composition root.
//!
//! This binary is the single place in the workspace that names concrete
//! provider and platform adapters. It is deliberately small: it loads the
//! redacted shared read runtime configuration, loads the complete stored access
//! credential, builds the authenticated Bungie profile gateway, the SQLite
//! projection store, and the Bungie transfer gateway, then serves the three read
//! tools plus the single intent-authorized `move_item` mutation over stdio.
//!
//! # Architecture
//!
//! The library crate (`destiny_mcp`) holds the RMCP server, the bounded MCP
//! schemas, and the injectable [`InventoryReadService`] and
//! [`MutationToolService`] seams, and never names an adapter. This binary is the
//! composition root that satisfies those seams with concrete adapters, which is
//! why `destiny-adapters` is a dependency of this crate rather than of the
//! library contracts.
//!
//! One profile gateway and one durable store are shared by both compositions
//! through `Arc<Mutex<..>>`, so the read path and the mutation path observe the
//! same SQLite connection and the same provider transport. `ReadToolServer`
//! additionally holds one outer operation guard for every read and mutation
//! request. All local inventory publication in this composition runs beneath
//! that guard, so no competing refresh can publish between fresh preflight and
//! the final ordered dispatch (or post-action verification). Individual adapter
//! locks remain short-lived and refresh does not reacquire the operation guard.
//! This is a process-local guarantee, not protection against external game
//! changes or another process writing the database. The mutation path is:
//!
//! `stdio -> rmcp server -> Box<dyn MutationToolService + Send> -> ComposedMutationService -> MutationService -> ProfileSyncService -> BungieProfileGateway / SqliteStore`,
//!
//! and the executor reaches the provider only through the shared
//! [`BungieTransferGateway`]. Mutation is composed by default: the advertised
//! tool set is the three reads plus `move_item`.
//!
//! # Protocol discipline
//!
//! Stdout carries JSON-RPC frames only. Startup failures and every runtime
//! failure are sanitized [`AppError`] values printed to stderr; no credential,
//! provider payload, or configuration value can appear in either stream.

use std::sync::{Arc, Mutex, MutexGuard};

use clap::Parser;
use destiny_adapters::sqlite::SqliteStore;
use destiny_adapters::{
    BungieProfileGateway, BungieTransferGateway, HttpProfileTransport, HttpTransferTransport,
    NativeCredentialStore, ProfileApiKey, SystemClock, TransferApiKey,
};
use destiny_app::{
    AppError, Clock, CredentialStore, DefinitionReplacementInput, DefinitionRepository,
    DefinitionSearchQuery, DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy,
    InstanceSearchQuery, InventoryRepository, InventorySnapshot, InventorySnapshotInput,
    ItemDefinition, ItemHash, ItemInstance, ItemInstanceId, MutationService, PlanId,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncReason, ProfileSyncService,
    QueryService, ReadRuntimeConfig, Resolution, SearchRepository, SyncMetadata,
};
use destiny_mcp::runtime::serve_stdio;
use destiny_mcp::{ComposedMutationService, QueryReadService, ReadToolServer};

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

/// Loads configuration, composes the shared read and mutation services, and
/// serves them over stdio.
async fn run() -> Result<(), AppError> {
    let config = ReadRuntimeConfig::from_env()?;
    let store = NativeCredentialStore::new()?;
    let server = compose_server(&config, store)?;
    serve_stdio(server).await
}

/// Builds the tool server from the runtime configuration and a credential store.
///
/// The credential is loaded first, so a missing or unusable credential fails as
/// a sanitized authorization error before any transport, database, or network
/// work is attempted. This makes the composition root's failure paths testable
/// with an injected store and no live keyring, provider, or database.
///
/// One profile gateway and one durable store are built and shared by the read
/// service and the mutation service, so the process holds a single provider
/// transport and a single SQLite connection. The read service keeps the exact
/// three read tools it always had; composing the mutation service appends the
/// single `move_item` tool and never changes read behavior.
fn compose_server<S: CredentialStore + Send + 'static>(
    config: &ReadRuntimeConfig,
    store: S,
) -> Result<ReadToolServer, AppError> {
    let (profile_source, transfer_source) = gateway_credentials(
        store,
        config
            .refresh_client()
            .map(|client| move || configured_refresher(client)),
        SystemClock::new(),
    )?;
    let policy = config.freshness_policy();
    // A separate local connection makes version resolution lazy: an empty
    // catalog is a typed tool error, not a startup failure or tool-time download.
    let version_store = SqliteStore::open(config.database_path())?;
    let version_config = config.clone();

    let profile_gateway = BungieProfileGateway::new(
        HttpProfileTransport::new()?,
        SystemClock::new(),
        profile_source,
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config
            .definition_version()
            .cloned()
            .unwrap_or_else(|| DefinitionVersion::new("unavailable")),
    )?
    .with_definition_version_resolver(move || {
        version_config.resolve_definition_version(&version_store)
    });
    let transfer = BungieTransferGateway::new(
        HttpTransferTransport::new()?,
        SystemClock::new(),
        transfer_source.clone(),
        TransferApiKey::new(config.api_key().expose_secret())?,
    )?;
    let equip = destiny_adapters::BungieEquipGateway::new(
        destiny_adapters::HttpEquipTransport::new()?,
        SystemClock::new(),
        transfer_source,
        destiny_adapters::EquipApiKey::new(config.api_key().expose_secret())?,
    )?;
    let repository = SqliteStore::open(config.database_path())?;
    Ok(assemble_server(
        profile_gateway,
        (transfer, equip),
        repository,
        policy,
        weekly_service(config)?,
        personal_public(config)?,
        SqliteStore::open(config.database_path())?,
    ))
}

fn configured_refresher(
    client: &destiny_app::RefreshClientConfig,
) -> Result<
    destiny_adapters::BungieTokenExchange<destiny_adapters::HttpTokenTransport, SystemClock>,
    AppError,
> {
    match client.auth_mode() {
        destiny_app::ClientAuthMode::Confidential(secret) => {
            destiny_adapters::BungieTokenExchange::confidential(
                client.client_id(),
                "ghostlink://oauth/callback",
                secret.clone(),
                destiny_adapters::HttpTokenTransport::new()?,
                SystemClock::new(),
            )
        }
        destiny_app::ClientAuthMode::Broker(base) => {
            destiny_adapters::BungieTokenExchange::brokered(
                client.client_id(),
                "ghostlink://oauth/callback",
                base,
                destiny_adapters::HttpTokenTransport::brokered(base)?,
                SystemClock::new(),
            )
        }
        destiny_app::ClientAuthMode::Public => Err(AppError::configuration()),
    }
}

type PersonalPublicGateway = destiny_adapters::manifest::BungieManifestGateway<
    destiny_adapters::manifest::HttpManifestTransport,
    SystemClock,
>;
fn personal_public(config: &ReadRuntimeConfig) -> Result<PersonalPublicGateway, AppError> {
    Ok(destiny_adapters::manifest::BungieManifestGateway::new(
        destiny_adapters::manifest::ManifestApiKey::new(config.api_key().expose_secret())?,
        destiny_adapters::manifest::HttpManifestTransport::new()?,
        SystemClock::new(),
    ))
}

type PublicWeeklyService = destiny_app::weekly_reset::WeeklyResetService<
    destiny_adapters::manifest::BungieManifestGateway<
        destiny_adapters::manifest::HttpManifestTransport,
        SystemClock,
    >,
    SqliteStore,
>;

fn weekly_service(config: &ReadRuntimeConfig) -> Result<PublicWeeklyService, AppError> {
    use destiny_adapters::manifest::{
        BungieManifestGateway, HttpManifestTransport, ManifestApiKey,
    };
    Ok(destiny_app::weekly_reset::WeeklyResetService::new(
        BungieManifestGateway::new(
            ManifestApiKey::new(config.api_key().expose_secret())?,
            HttpManifestTransport::new()?,
            SystemClock::new(),
        ),
        SqliteStore::open(config.database_path())?,
    ))
}

/// Creates one session, then shares its lock-backed cache between both gateways.
type GatewayCredentials<S, R, C> = (
    destiny_app::ConfiguredCredentialSource<S, R, C>,
    destiny_app::ConfiguredCredentialSource<S, R, C>,
);

fn gateway_credentials<S, R, C, F>(
    store: S,
    refresh: Option<F>,
    clock: C,
) -> Result<GatewayCredentials<S, R, C>, AppError>
where
    S: CredentialStore,
    R: destiny_app::TokenRefresh,
    C: Clock,
    F: FnOnce() -> Result<R, AppError>,
{
    let source = destiny_app::ConfiguredCredentialSource::new(store, refresh, clock)?;
    Ok((source.clone(), source))
}

/// Wires the concrete adapters into one read-plus-mutation tool server.
///
/// The read service and the mutation service share the profile gateway and the
/// durable store, and the mutation service is always composed, so the product
/// server advertises the single `move_item` tool by default. A plan is created
/// and executed inside one request, so the bounded maximum plan lifetime is the
/// safest accepted window.
fn assemble_server<C: destiny_app::CredentialSource + Send + 'static>(
    profile_gateway: BungieProfileGateway<HttpProfileTransport, SystemClock, C>,
    ports: (
        BungieTransferGateway<HttpTransferTransport, SystemClock, C>,
        destiny_adapters::BungieEquipGateway<destiny_adapters::HttpEquipTransport, SystemClock, C>,
    ),
    repository: SqliteStore,
    policy: FreshnessPolicy,
    weekly: PublicWeeklyService,
    personal_public: PersonalPublicGateway,
    personal_repository: SqliteStore,
) -> ReadToolServer {
    let gateway = SharedGateway::new(profile_gateway);
    let repository = SharedRepository::new(repository);

    let read_sync = ProfileSyncService::new_with_definitions(gateway.clone(), repository.clone());
    let read_service =
        QueryReadService::new(QueryService::new(read_sync), SystemClock::new(), policy);

    let read_service = destiny_mcp::service::PersonalWeeklyReadService::new(
        destiny_app::WeeklyPersonalService::new(
            personal_public,
            gateway.clone(),
            personal_repository,
        ),
        read_service,
    );

    let read_service = destiny_mcp::service::WeeklyReadService::new(weekly, read_service, |t| {
        destiny_adapters::format_reset_timestamp(t, false)
    });

    let read_service = destiny_mcp::service::ArtifactReadService::new(
        destiny_app::ArtifactService::new(ProfileSyncService::new_with_definitions(
            gateway.clone(),
            repository.clone(),
        )),
        read_service,
        SystemClock::new(),
        policy,
    );

    let read_service = destiny_mcp::service::LoadoutsReadService::new(
        destiny_app::LoadoutService::new(ProfileSyncService::new_with_definitions(
            gateway.clone(),
            repository.clone(),
        )),
        read_service,
        SystemClock::new(),
        policy,
    );

    let mutations = MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
        SystemPlanIds::new(SystemClock::new()),
    );
    let mutation_service = ComposedMutationService::new(
        mutations,
        ports.0,
        SystemClock::new(),
        policy,
        PlanLifetime::MAX,
    )
    .with_equip(ports.1);

    // Every publishing read decorator and both mutations enter the same outer
    // operation boundary. Do not expose a second unguarded dispatch path for
    // these shared repositories: revision comparison alone is not exclusion.
    ReadToolServer::with_mutation(Box::new(read_service), Box::new(mutation_service))
}

/// Loads the complete stored credential, failing closed when it is absent.
///
/// A missing entry is an explicit [`ErrorCategory::AuthorizationRequired`]
/// failure: the process never falls back to an environment value, a fixture, or
/// a plaintext file. A store error propagates unchanged, so a legacy
/// raw-material entry stays [`ErrorCategory::ReauthorizationRequired`] and a
/// corrupt or unavailable store stays [`ErrorCategory::CredentialStoreUnavailable`].
#[cfg(test)]
fn load_credential<S: CredentialStore>(
    store: &S,
) -> Result<destiny_app::AccessCredential, AppError> {
    match store.load()? {
        Some(credential) => Ok(credential),
        None => Err(AppError::new(ErrorCategory::AuthorizationRequired)),
    }
}

/// The production opaque plan identifier generator.
///
/// It combines the whole-second wall clock with a process-local monotonic
/// sequence. Within one process the pair is unique: the sequence never repeats
/// and never wraps (an exhausted sequence fails closed), so two plans created in
/// the same process never share an identifier even within one second. The value
/// is request-scoped and is never persisted or compared across processes, so it
/// makes no cross-restart uniqueness claim: a later process that starts in the
/// same wall-clock second begins its sequence again. The identifier is internal
/// to one request and is redacted everywhere it could be rendered, so it needs
/// no randomness to be safe; `PlanLifetime` already bounds how long it matters.
///
/// The clock is a type parameter so a test can inject a deterministic source;
/// production always builds it with [`SystemClock`].
struct SystemPlanIds<C> {
    clock: C,
    sequence: u64,
}

impl<C: Clock> SystemPlanIds<C> {
    fn new(clock: C) -> Self {
        Self { clock, sequence: 0 }
    }
}

impl<C: Clock> PlanIdGenerator for SystemPlanIds<C> {
    fn next_plan_id(&mut self) -> Result<PlanId, AppError> {
        // The sequence is reserved before the clock is read, so an exhausted
        // generator fails closed without spending a clock read, and a clock
        // failure propagates as its own sanitized category.
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| AppError::new(ErrorCategory::PlanIdExhausted))?;
        self.sequence = sequence;
        let seconds = self.clock.now()?.seconds();
        Ok(PlanId::new(format!("plan-{seconds}-{sequence}")))
    }
}

/// Shares one profile gateway between the read and mutation compositions.
///
/// Both application services own their synchronization service by value, so the
/// composition root reuses the one provider transport through a mutex. The lock
/// is only ever held for one synchronous provider call, and no path nests it
/// with the repository lock. The wrapper is generic over the gateway so the
/// production composition still shares the one concrete adapter while a test
/// can inject a recording gateway.
///
/// The scoped read methods forward the exact [`ProfileSyncReason`] to the locked
/// inner adapter. Relying on the [`ProfileGateway`] defaults here would discard
/// the reason and pin every MCP read to the conservative one-shot scope, so the
/// reason-scoped retry policy could never reach an ordinary MCP read. Forwards
/// are deliberate for both scoped methods, mirroring the adapter's own
/// override, so a mutation observation stays one-shot while an ordinary read
/// repeats a bounded transient failure.
struct SharedGateway<G> {
    inner: Arc<Mutex<G>>,
}

impl<G> Clone for SharedGateway<G> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<G> SharedGateway<G> {
    fn new(gateway: G) -> Self {
        Self {
            inner: Arc::new(Mutex::new(gateway)),
        }
    }

    /// Acquires the shared gateway for one synchronous call.
    ///
    /// A poisoned lock can only follow a panic in an unrelated thread that was
    /// holding the gateway; it is reported as a sanitized local-storage failure
    /// rather than propagating a panic payload.
    fn lock(&self) -> Result<MutexGuard<'_, G>, AppError> {
        self.inner
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))
    }
}

impl<G: destiny_app::WeeklyProfileGateway> destiny_app::WeeklyProfileGateway for SharedGateway<G> {
    fn load_character_weekly(
        &mut self,
        membership: &DestinyMembership,
        character: &destiny_app::CharacterId,
    ) -> Result<destiny_app::CharacterWeeklySnapshot, AppError> {
        self.lock()?.load_character_weekly(membership, character)
    }
}

impl<G: destiny_app::ArtifactGateway> destiny_app::ArtifactGateway for SharedGateway<G> {
    fn load_artifact(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<destiny_app::ArtifactSnapshot, AppError> {
        self.lock()?.load_artifact(membership)
    }
}

impl<G: destiny_app::LoadoutGateway> destiny_app::LoadoutGateway for SharedGateway<G> {
    fn load_loadouts(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<destiny_app::LoadoutSnapshot, AppError> {
        self.lock()?.load_loadouts(membership)
    }
}

impl<G: ProfileGateway> ProfileGateway for SharedGateway<G> {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        self.lock()?.resolve_membership()
    }

    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.lock()?.load_inventory(membership)
    }

    fn resolve_membership_scoped(
        &mut self,
        reason: ProfileSyncReason,
    ) -> Result<DestinyMembership, AppError> {
        self.lock()?.resolve_membership_scoped(reason)
    }

    fn load_inventory_scoped(
        &mut self,
        membership: &DestinyMembership,
        reason: ProfileSyncReason,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.lock()?.load_inventory_scoped(membership, reason)
    }
}

/// Shares one durable projection store between the read and mutation
/// compositions.
///
/// A single SQLite connection backs every repository trait both services need,
/// so a committed refresh from either path is immediately visible to the other
/// and the two writers can never contend on separate connections. The lock is
/// only ever held for one synchronous repository call.
#[derive(Clone)]
struct SharedRepository {
    inner: Arc<Mutex<SqliteStore>>,
}

impl SharedRepository {
    fn new(store: SqliteStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
        }
    }

    /// Acquires the shared store for one synchronous repository call.
    fn lock(&self) -> Result<MutexGuard<'_, SqliteStore>, AppError> {
        self.inner
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))
    }
}

impl InventoryRepository for SharedRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        self.lock()?.load_current()
    }

    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        self.lock()?.replace_current(input)
    }
}

impl SearchRepository for SharedRepository {
    fn name_catalog(
        &self,
        kind: &str,
    ) -> Result<destiny_app::name_matching::NameCatalog, AppError> {
        self.lock()?.name_catalog(kind)
    }
    fn search_definitions(
        &self,
        query: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        self.lock()?.search_definitions(query)
    }

    fn search_instances(&self, query: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
        self.lock()?.search_instances(query)
    }

    fn lookup_instance(
        &self,
        instance_id: &ItemInstanceId,
    ) -> Result<Resolution<ItemInstance>, AppError> {
        self.lock()?.lookup_instance(instance_id)
    }
}

impl destiny_app::StatDefinitionRepository for SharedRepository {
    fn lookup_stat(&self, hash: u32) -> Result<Option<destiny_app::StatDefinition>, AppError> {
        self.lock()?.lookup_stat(hash)
    }
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
        self.lock()?.stat_hashes_by_name(name)
    }
    fn stat_definition_count(&self) -> Result<usize, AppError> {
        self.lock()?.stat_definition_count()
    }
}

impl destiny_app::WorldDefinitionRepository for SharedRepository {
    fn lookup_milestone(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_app::MilestoneDefinition>, AppError> {
        self.lock()?.lookup_milestone(hash)
    }
    fn lookup_activity(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_app::ActivityDefinition>, AppError> {
        self.lock()?.lookup_activity(hash)
    }
    fn lookup_activity_modifier(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_app::ActivityModifierDefinition>, AppError> {
        self.lock()?.lookup_activity_modifier(hash)
    }
    fn lookup_objective(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_app::ObjectiveDefinition>, AppError> {
        self.lock()?.lookup_objective(hash)
    }
    fn lookup_activity_type(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_app::ActivityTypeDefinition>, AppError> {
        self.lock()?.lookup_activity_type(hash)
    }
    fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
        self.lock()?.lookup_loadout_name(hash)
    }
    fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
        self.lock()?.world_definition_counts()
    }
    fn publish_manifest(
        &mut self,
        items: DefinitionReplacementInput,
        world: destiny_app::WorldDefinitionsAcquisition,
    ) -> Result<SyncMetadata, AppError> {
        self.lock()?.publish_manifest(items, world)
    }
}

impl DefinitionRepository for SharedRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.lock()?.current_definition_version()
    }

    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        self.lock()?.lookup_definition(hash)
    }

    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        self.lock()?.current_metadata()
    }

    fn replace_definitions(
        &mut self,
        replacement: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.lock()?.replace_definitions(replacement)
    }
}

#[cfg(test)]
mod tests {
    use super::personal_public;
    use super::{SystemPlanIds, compose_server, load_credential};
    use destiny_app::test_support::{FailingCredentialStore, FakeClock, InMemoryCredentialStore};
    use destiny_app::{
        AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial,
        CredentialStore, ErrorCategory, PlanIdGenerator, ReadRuntimeConfig, Timestamp,
    };
    use std::collections::HashMap;

    const API_KEY_SENTINEL: &str = "api-key-sentinel";
    const CREDENTIAL_SENTINEL: &str = "credential-sentinel";

    fn values(database_path: &str) -> HashMap<String, String> {
        HashMap::from([
            ("BUNGIE_API_KEY".into(), API_KEY_SENTINEL.into()),
            ("DESTINY_DATABASE_PATH".into(), database_path.into()),
            ("DESTINY_DEFINITION_VERSION".into(), "manifest-v1".into()),
            ("DESTINY_FRESHNESS_TTL_SECONDS".into(), "900".into()),
        ])
    }

    fn config() -> ReadRuntimeConfig {
        ReadRuntimeConfig::from_values(&values("/tmp/destiny-mcp-test.db"))
            .expect("valid configuration")
    }

    fn credential() -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_700_000_000),
            },
            material: CredentialMaterial::from_bytes(CREDENTIAL_SENTINEL.as_bytes().to_vec()),
            refresh_grant: None,
        }
    }

    #[test]
    fn composed_gateway_credentials_refresh_once_across_profile_and_transfer() {
        use destiny_adapters::profile_gateway::{
            ProfileHttpRequest, ProfileHttpResponse, ProfileTransport, ProfileTransportError,
        };
        use destiny_adapters::transfer::{
            TransferHttpRequest, TransferHttpResponse, TransferTransport, TransferTransportError,
        };
        use destiny_app::{RefreshGrant, RefreshToken, TokenRefresh, TransferGateway};
        struct Offline;
        impl ProfileTransport for Offline {
            fn get(
                &mut self,
                _: &ProfileHttpRequest<'_>,
            ) -> Result<ProfileHttpResponse, ProfileTransportError> {
                Err(ProfileTransportError::Unavailable)
            }
        }
        impl TransferTransport for Offline {
            fn post(
                &mut self,
                _: &TransferHttpRequest<'_>,
            ) -> Result<TransferHttpResponse, TransferTransportError> {
                Err(TransferTransportError::NotDispatched)
            }
        }
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Refresh(Arc<AtomicUsize>);
        impl TokenRefresh for Refresh {
            fn refresh(&mut self, _: &RefreshToken) -> Result<AccessCredential, AppError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                let mut credential = credential();
                credential.metadata.expires_at = Timestamp::new(1000);
                Ok(credential)
            }
        }
        let mut store = InMemoryCredentialStore::default();
        let mut old = credential();
        old.metadata.expires_at = Timestamp::new(150);
        old.refresh_grant = Some(RefreshGrant {
            token: RefreshToken::from_bytes(b"refresh-sentinel").unwrap(),
            expires_at: Timestamp::new(900),
        });
        store.save(old).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let (profile, transfer) = super::gateway_credentials(
            store,
            Some(|| Ok(Refresh(count.clone()))),
            FakeClock::fixed(Timestamp::new(100)),
        )
        .unwrap();
        let mut profile = super::BungieProfileGateway::new(
            Offline,
            FakeClock::fixed(Timestamp::new(100)),
            profile,
            super::ProfileApiKey::new(API_KEY_SENTINEL).unwrap(),
            destiny_app::DefinitionVersion::new("manifest-v1"),
        )
        .unwrap();
        assert!(destiny_app::ProfileGateway::resolve_membership(&mut profile).is_err());
        let mut transfer = super::BungieTransferGateway::new(
            Offline,
            FakeClock::fixed(Timestamp::new(100)),
            transfer,
            super::TransferApiKey::new(API_KEY_SENTINEL).unwrap(),
        )
        .unwrap();
        let membership = destiny_app::DestinyMembership::new(
            destiny_app::MembershipType::new(3),
            destiny_app::MembershipId::new("123"),
        )
        .unwrap();
        let request = destiny_app::TransferRequest::new(
            membership,
            destiny_app::ItemInstanceId::new("456"),
            destiny_app::ItemHash::new(789),
            1,
            destiny_app::ItemLocation::Character(destiny_app::CharacterId::new("123")),
            destiny_app::ItemLocation::Vault,
        )
        .unwrap();
        assert!(transfer.transfer(&request).is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn absent_manifest_without_override_returns_typed_tool_error_offline() {
        use destiny_adapters::profile_gateway::{
            ProfileHttpRequest, ProfileHttpResponse, ProfileTransport, ProfileTransportError,
        };
        use destiny_mcp::service::InventoryReadService;
        struct Offline;
        impl ProfileTransport for Offline {
            fn get(
                &mut self,
                _: &ProfileHttpRequest<'_>,
            ) -> Result<ProfileHttpResponse, ProfileTransportError> {
                panic!("an absent manifest must not make a profile request");
            }
        }
        let mut values = values("/unreachable/destiny.db");
        values.remove("DESTINY_DEFINITION_VERSION");
        let config = ReadRuntimeConfig::from_values(&values).unwrap();
        let version_store = destiny_adapters::sqlite::SqliteStore::open_in_memory().unwrap();
        let gateway = destiny_adapters::BungieProfileGateway::new(
            Offline,
            FakeClock::fixed(Timestamp::new(100)),
            destiny_app::StaticCredentialSource::new(credential()),
            destiny_adapters::ProfileApiKey::new(API_KEY_SENTINEL).unwrap(),
            destiny_app::DefinitionVersion::new("unavailable"),
        )
        .unwrap()
        .with_definition_version_resolver(move || {
            config.resolve_definition_version(&version_store)
        });
        let store = destiny_adapters::sqlite::SqliteStore::open_in_memory().unwrap();
        let mut service = destiny_mcp::QueryReadService::new(
            destiny_app::QueryService::new(destiny_app::ProfileSyncService::new_with_definitions(
                gateway, store,
            )),
            FakeClock::fixed(Timestamp::new(100)),
            destiny_app::FreshnessPolicy { ttl_seconds: 900 },
        );
        let error = service
            .list_characters(&destiny_mcp::service::ListCharactersRequest)
            .unwrap_err();
        assert_eq!(error.code(), "definitions_unavailable");
        assert_eq!(
            error.message(),
            "definitions not loaded; run ghostlink sync-manifest"
        );
    }

    #[test]
    fn broker_config_selects_shared_refreshing_gateway_credentials() {
        let mut values = values("/unreachable/destiny.db");
        values.insert("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into());
        values.insert(
            "GHOSTLINK_TOKEN_BROKER_URL".into(),
            "https://broker.test".into(),
        );
        let config = ReadRuntimeConfig::from_values(&values).unwrap();
        let mut store = InMemoryCredentialStore::default();
        store.save(credential()).unwrap();
        let (profile, transfer) = super::gateway_credentials(
            store,
            config
                .refresh_client()
                .map(|client| move || super::configured_refresher(client)),
            FakeClock::fixed(Timestamp::new(100)),
        )
        .unwrap();
        assert!(matches!(
            profile,
            destiny_app::ConfiguredCredentialSource::Refreshing(_)
        ));
        assert!(matches!(
            transfer,
            destiny_app::ConfiguredCredentialSource::Refreshing(_)
        ));
        assert_eq!(
            compose_server(&config, InMemoryCredentialStore::default())
                .unwrap_err()
                .category(),
            ErrorCategory::AuthorizationRequired
        );
    }

    #[test]
    fn confidential_missing_credential_stops_before_token_transport() {
        let mut values = values("/unreachable/destiny.db");
        values.insert("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into());
        values.insert("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into());
        let config = ReadRuntimeConfig::from_values(&values).unwrap();
        assert_eq!(
            compose_server(&config, InMemoryCredentialStore::default())
                .unwrap_err()
                .category(),
            ErrorCategory::AuthorizationRequired
        );
    }

    #[test]
    fn shared_gateway_forwards_the_exact_sync_reason_to_the_inner_scoped_reads() {
        use super::SharedGateway;
        use destiny_app::{
            DefinitionVersion, DestinyMembership, InventorySnapshotInput, MembershipId,
            MembershipType, ProfileGateway, ProfileSyncReason, SourceMarker,
        };
        use std::sync::{Arc, Mutex};

        /// Records the reason of every scoped read. The unscoped methods are
        /// unreachable on purpose: if the wrapper fell back to the trait
        /// default, it would call them and the reason would be silently lost.
        struct RecordingGateway {
            reasons: Arc<Mutex<Vec<ProfileSyncReason>>>,
        }

        fn membership() -> DestinyMembership {
            DestinyMembership::new(
                MembershipType::new(3),
                MembershipId::new("4611686018484639825"),
            )
            .expect("valid membership")
        }

        fn inventory_input(membership: &DestinyMembership) -> InventorySnapshotInput {
            InventorySnapshotInput {
                plug_set_entries: Vec::new(),
                membership_id: membership.membership_id().clone(),
                membership_type: membership.membership_type(),
                definition_version: DefinitionVersion::new("manifest-v1"),
                checked_at: Timestamp::new(1_724_000_000),
                source: SourceMarker::Profile,
                characters: Vec::new(),
                items: Vec::new(),
            }
        }

        impl ProfileGateway for RecordingGateway {
            fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                unreachable!("the scoped read must not fall back to the unscoped default")
            }

            fn load_inventory(
                &mut self,
                _membership: &DestinyMembership,
            ) -> Result<InventorySnapshotInput, AppError> {
                unreachable!("the scoped read must not fall back to the unscoped default")
            }

            fn resolve_membership_scoped(
                &mut self,
                reason: ProfileSyncReason,
            ) -> Result<DestinyMembership, AppError> {
                self.reasons.lock().expect("reason log").push(reason);
                Ok(membership())
            }

            fn load_inventory_scoped(
                &mut self,
                membership: &DestinyMembership,
                reason: ProfileSyncReason,
            ) -> Result<InventorySnapshotInput, AppError> {
                self.reasons.lock().expect("reason log").push(reason);
                Ok(inventory_input(membership))
            }
        }

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let mut gateway = SharedGateway::new(RecordingGateway {
            reasons: Arc::clone(&reasons),
        });
        let membership = gateway
            .resolve_membership_scoped(ProfileSyncReason::Preflight)
            .expect("scoped resolve forwards to the inner adapter");
        gateway
            .load_inventory_scoped(&membership, ProfileSyncReason::Ordinary)
            .expect("scoped load forwards to the inner adapter");

        assert_eq!(
            *reasons.lock().expect("reason log"),
            [ProfileSyncReason::Preflight, ProfileSyncReason::Ordinary],
            "the wrapper must forward the exact reason to the locked inner adapter"
        );
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
        let error = compose_server(&config(), InMemoryCredentialStore::default())
            .expect_err("missing credential stops composition");
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        assert!(!error.to_string().contains(API_KEY_SENTINEL));
    }

    #[test]
    fn composition_fails_before_any_adapter_work_for_a_legacy_credential() {
        let store = FailingCredentialStore {
            error: destiny_app::AppError::new(ErrorCategory::ReauthorizationRequired),
        };
        let error =
            compose_server(&config(), store).expect_err("legacy credential stops composition");
        assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);
    }

    #[test]
    fn composition_fails_before_any_adapter_work_for_an_unavailable_or_unsupported_store() {
        // An unusable (`CredentialStoreUnavailable`) or operation-unsupported
        // backend collapses to the same sanitized category at the composition
        // root, so composition stops before any transport, database, or network
        // work and never selects a weaker store.
        let store = FailingCredentialStore {
            error: destiny_app::AppError::new(ErrorCategory::CredentialStoreUnavailable),
        };
        let error =
            compose_server(&config(), store).expect_err("an unavailable store stops composition");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
        assert!(!error.to_string().contains(API_KEY_SENTINEL));
    }

    #[test]
    fn assembly_advertises_the_reads_and_the_default_enabled_mutation() {
        // The assembly seam builds the shared adapters offline (an in-memory
        // store and the HTTP clients without a connection) and the product
        // server advertises the three reads plus `move_item`.
        use super::assemble_server;
        use destiny_adapters::sqlite::SqliteStore;
        use destiny_adapters::{
            BungieProfileGateway, BungieTransferGateway, HttpProfileTransport,
            HttpTransferTransport, ProfileApiKey, SystemClock, TransferApiKey,
        };
        use destiny_app::{DefinitionVersion, FreshnessPolicy};

        let credential = credential();
        let profile_gateway = BungieProfileGateway::new(
            HttpProfileTransport::new().expect("profile transport"),
            SystemClock::new(),
            destiny_app::StaticCredentialSource::new(credential.clone()),
            ProfileApiKey::new(API_KEY_SENTINEL).expect("valid profile API key"),
            DefinitionVersion::new("manifest-v1"),
        )
        .expect("profile gateway");
        let transfer = BungieTransferGateway::new(
            HttpTransferTransport::new().expect("transfer transport"),
            SystemClock::new(),
            destiny_app::StaticCredentialSource::new(credential.clone()),
            TransferApiKey::new(API_KEY_SENTINEL).expect("valid transfer API key"),
        )
        .expect("transfer gateway");
        let store = SqliteStore::open_in_memory().expect("in-memory store");

        let equip = destiny_adapters::BungieEquipGateway::new(
            destiny_adapters::HttpEquipTransport::new().unwrap(),
            SystemClock::new(),
            destiny_app::StaticCredentialSource::new(credential),
            destiny_adapters::EquipApiKey::new(API_KEY_SENTINEL).unwrap(),
        )
        .unwrap();
        let server = assemble_server(
            profile_gateway,
            (transfer, equip),
            store,
            FreshnessPolicy { ttl_seconds: 900 },
            destiny_app::weekly_reset::WeeklyResetService::new(
                destiny_adapters::manifest::BungieManifestGateway::new(
                    destiny_adapters::manifest::ManifestApiKey::new(API_KEY_SENTINEL).unwrap(),
                    destiny_adapters::manifest::HttpManifestTransport::new().unwrap(),
                    SystemClock::new(),
                ),
                SqliteStore::open_in_memory().unwrap(),
            ),
            personal_public(&config()).unwrap(),
            SqliteStore::open_in_memory().unwrap(),
        );
        let tools = server.discover_tools().tools;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "list_characters",
                "search_inventory",
                "inspect_item",
                "get_subclass",
                "get_artifact",
                "get_loadouts",
                "weekly_reset",
                "weekly",
                "get_subclass_options",
                "move_item",
                "equip_item"
            ],
            "mutation must be default-enabled in the product composition"
        );
    }

    #[test]
    fn plan_ids_are_unique_within_one_second() {
        // A deterministic clock freezes `now`, so only the process-local
        // sequence distinguishes the two identifiers.
        let mut ids = SystemPlanIds::new(FakeClock::fixed(Timestamp::new(1_724_000_000)));
        let first = ids.next_plan_id().expect("the first plan id");
        let second = ids.next_plan_id().expect("the second plan id");
        assert_ne!(first, second);
        assert_eq!(first.as_str(), "plan-1724000000-1");
        assert_eq!(second.as_str(), "plan-1724000000-2");
    }

    #[test]
    fn an_exhausted_plan_id_sequence_fails_closed() {
        let mut ids = SystemPlanIds {
            clock: FakeClock::fixed(Timestamp::new(1_724_000_000)),
            sequence: u64::MAX,
        };
        let error = ids
            .next_plan_id()
            .expect_err("an exhausted sequence fails closed");
        assert_eq!(error.category(), ErrorCategory::PlanIdExhausted);
    }

    #[test]
    fn a_clock_failure_propagates_its_sanitized_category() {
        /// A deterministic clock that always fails, mirroring the adapter's
        /// pre-epoch/overflow failure.
        struct FailingClock;

        impl Clock for FailingClock {
            fn now(&self) -> Result<Timestamp, AppError> {
                Err(AppError::new(ErrorCategory::ClockOverflow))
            }
        }

        let mut ids = SystemPlanIds::new(FailingClock);
        let error = ids.next_plan_id().expect_err("a clock failure propagates");
        assert_eq!(error.category(), ErrorCategory::ClockOverflow);
    }
}
