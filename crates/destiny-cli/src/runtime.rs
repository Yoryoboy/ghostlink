//! Production read-runtime composition for the CLI.
//!
//! The process boundary in [`crate::runner`] first parses the command line and
//! semantically validates the read request. Only after that succeeds does it
//! ask a [`RuntimeFactory`] for the runtime, and a valid request is dispatched
//! through the existing runner path rather than any fallback. An invalid
//! request can therefore never read configuration, open a credential store,
//! open the database, read a clock, build a transport, or touch the network.
//!
//! [`compose`] builds the runtime in one exact fail-fast order:
//!
//! 1. [`ReadRuntimeConfig::from_env`] reads validated configuration;
//! 2. the native credential store is built and the complete stored
//!    [`AccessCredential`] is loaded, and a missing credential is reported as
//!    the stable [`ErrorCategory::AuthorizationRequired`] outcome. Confidential
//!    mode builds a refreshing source only after that load; public mode stays static;
//! 3. the production [`SystemClock`] is built and read exactly once;
//! 4. the hardened [`HttpProfileTransport`] is built;
//! 5. the [`ProfileApiKey`] is derived from the validated [`ReadApiKey`];
//! 6. the durable [`SqliteStore`] is opened at the configured path;
//! 7. [`BungieProfileGateway`] is built with the override or stored Manifest version;
//! 8. [`ProfileSyncService`] and then [`QueryService`] are assembled.
//!
//! The one clock reading supplies the [`ReadFreshness`] the runner forwards to
//! the shared services together with the configured `FreshnessPolicy`:
//! freshness, authorization, and synchronization decisions stay in the shared
//! application services. Every failure is a sanitized, value-free
//! [`AppError`]; no configuration, credential, provider value, database path,
//! or definition version is ever rendered or echoed.
//!
//! The fallible acquisitions live behind the [`ReadFoundations`] seam so the
//! ordering and the sanitized mapping are testable without an environment, a
//! keyring, or a network, and the whole runtime lives behind [`RuntimeFactory`]
//! so the boundary can be driven with a supplied service.

use std::fmt;

use destiny_adapters::NativeCredentialStore;
use destiny_adapters::SystemClock;
use destiny_adapters::profile_gateway::{
    BungieProfileGateway, HttpProfileTransport, ProfileApiKey, ProfileTransport,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::{
    AccessCredential, AppError, Clock, CredentialStore, DefinitionRepository, ErrorCategory,
    InventoryRepository, ProfileSyncService, QueryOperations, QueryService, ReadRuntimeConfig,
    SearchRepository,
};

use crate::command::CliError;
use crate::runner::ReadFreshness;

/// The fallible foundation acquisitions the production runtime performs.
///
/// The methods are called in declaration order by [`compose`], and each one
/// reports a sanitized [`AppError`]. The associated types let a test drive the
/// composition with scripted foundations while the production implementation
/// uses the real adapter types.
pub trait ReadFoundations {
    /// Per-request credentials, selected by validated configuration.
    type Source: destiny_app::CredentialSource;
    /// The transport the gateway sends over.
    type Transport: ProfileTransport;
    /// The clock the gateway and the freshness input read.
    type Clock: Clock;
    /// The durable repository behind the sync and query services.
    type Repository: InventoryRepository
        + SearchRepository
        + DefinitionRepository
        + destiny_app::StatDefinitionRepository
        + destiny_app::WorldDefinitionRepository;

    /// Reads and validates the shared read runtime configuration.
    fn config(&mut self) -> Result<ReadRuntimeConfig, AppError>;

    /// Loads the complete stored access credential.
    fn credential_source(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Source, AppError>;

    /// Builds the production clock. Building it is not a clock reading.
    fn clock(&mut self) -> Result<Self::Clock, AppError>;

    /// Builds the hardened profile transport.
    fn transport(&mut self) -> Result<Self::Transport, AppError>;

    /// Opens the durable projection store for the validated configuration.
    fn store(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Repository, AppError>;
}

/// Loads the stored credential or fails with the stable authorization outcome.
///
/// A present credential is returned unchanged. A missing credential is the
/// explicit [`ErrorCategory::AuthorizationRequired`] outcome rather than a
/// silent empty runtime, and a credential-store failure propagates its own
/// sanitized category. No stored byte is ever rendered.
fn load_required_credential<S>(store: &S) -> Result<AccessCredential, AppError>
where
    S: CredentialStore,
{
    match store.load()? {
        Some(credential) => Ok(credential),
        None => Err(AppError::new(ErrorCategory::AuthorizationRequired)),
    }
}

/// The production foundation acquisitions over the real adapters.
///
/// Every method is the thin adapter call the exact composition order requires;
/// it holds no state, so constructing it is infallible.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionFoundations;

impl ReadFoundations for ProductionFoundations {
    type Source = destiny_app::ConfiguredCredentialSource<
        NativeCredentialStore,
        destiny_adapters::BungieTokenExchange<destiny_adapters::HttpTokenTransport, SystemClock>,
        SystemClock,
    >;
    type Transport = HttpProfileTransport;
    type Clock = SystemClock;
    type Repository = SqliteStore;

    fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
        ReadRuntimeConfig::from_env()
    }

    fn credential_source(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Source, AppError> {
        let store = NativeCredentialStore::new()?;
        compose_credentials(config, store, configured_refresher, SystemClock::new())
    }

    fn clock(&mut self) -> Result<SystemClock, AppError> {
        Ok(SystemClock::new())
    }

    fn transport(&mut self) -> Result<HttpProfileTransport, AppError> {
        HttpProfileTransport::new()
    }

    fn store(&mut self, config: &ReadRuntimeConfig) -> Result<SqliteStore, AppError> {
        SqliteStore::open(config.database_path())
    }
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

/// Selects the per-request source before any profile or database acquisition.
fn compose_credentials<S, R, C, F>(
    config: &ReadRuntimeConfig,
    store: S,
    refresh: F,
    clock: C,
) -> Result<destiny_app::ConfiguredCredentialSource<S, R, C>, AppError>
where
    S: CredentialStore,
    R: destiny_app::TokenRefresh,
    C: Clock,
    F: FnOnce(&destiny_app::RefreshClientConfig) -> Result<R, AppError>,
{
    if let Some(client) = config.refresh_client() {
        destiny_app::ConfiguredCredentialSource::new(store, Some(|| refresh(client)), clock)
    } else {
        Ok(destiny_app::ConfiguredCredentialSource::Static(
            destiny_app::StaticCredentialSource::new(load_required_credential(&store)?),
        ))
    }
}

/// The assembled read service type for one set of foundations.
pub type ComposedReadService<F> = QueryService<
    BungieProfileGateway<
        <F as ReadFoundations>::Transport,
        <F as ReadFoundations>::Clock,
        <F as ReadFoundations>::Source,
    >,
    <F as ReadFoundations>::Repository,
>;

/// One ready-to-dispatch read service plus the freshness inputs it must receive.
///
/// The service is the shared [`QueryService`] behind [`QueryOperations`], and
/// the freshness carries the configured policy and the single clock reading.
/// The runner forwards both unchanged.
pub struct ReadRuntime<S> {
    service: S,
    freshness: ReadFreshness,
}

impl<S> ReadRuntime<S> {
    /// Pairs one service with the freshness inputs it will be dispatched with.
    pub fn new(service: S, freshness: ReadFreshness) -> Self {
        Self { service, freshness }
    }

    /// The mutable service the runner dispatches exactly one read through.
    pub fn service_mut(&mut self) -> &mut S {
        &mut self.service
    }

    /// The caller-supplied freshness inputs for this runtime.
    pub fn freshness(&self) -> ReadFreshness {
        self.freshness
    }
}

impl<S> fmt::Debug for ReadRuntime<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadRuntime")
            .field("freshness", &self.freshness)
            .finish_non_exhaustive()
    }
}

/// CLI-local explicit sync result; metadata is returned only after publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSyncReport {
    pub metadata: destiny_app::SyncMetadata,
    pub previous_revision: Option<destiny_app::SyncRevision>,
}

/// Explicit profile acquisition without expanding the shared read API.
pub trait ProfileSyncOperations {
    fn run_profile_sync(
        &mut self,
        policy: destiny_app::FreshnessPolicy,
        now: destiny_app::Timestamp,
    ) -> Result<ProfileSyncReport, AppError>;
}

impl<G, R> ProfileSyncOperations for QueryService<G, R>
where
    G: destiny_app::ProfileGateway,
    R: InventoryRepository
        + SearchRepository
        + DefinitionRepository
        + destiny_app::StatDefinitionRepository,
{
    fn run_profile_sync(
        &mut self,
        policy: destiny_app::FreshnessPolicy,
        now: destiny_app::Timestamp,
    ) -> Result<ProfileSyncReport, AppError> {
        let previous_revision = self
            .repository()
            .load_current()?
            .map(|snapshot| snapshot.metadata.revision);
        let outcome = self
            .sync_mut()
            .sync(destiny_app::ProfileSyncReason::Forced, policy, now)?;
        match outcome {
            destiny_app::ProfileSyncOutcome::Refreshed { metadata } => Ok(ProfileSyncReport {
                metadata: destiny_app::SyncMetadata {
                    dataset: destiny_app::Dataset::ProfileInventory,
                    revision: metadata.revision,
                    checked_at: metadata.checked_at,
                    source: metadata.source,
                    definition_version: metadata.definition_version,
                },
                previous_revision,
            }),
            destiny_app::ProfileSyncOutcome::SkippedFresh { .. }
            | destiny_app::ProfileSyncOutcome::DiscardedNotNewer { .. } => {
                Err(AppError::new(ErrorCategory::StaleState))
            }
        }
    }
}

/// The injectable seam the process boundary uses to obtain a read runtime.
///
/// Production supplies the real composition; tests supply a scripted service,
/// policy, and time. A build failure is a stable, sanitized [`CliError`], and a
/// failure is the only way [`RuntimeFactory::build`] can avoid returning a
/// dispatchable runtime.
pub trait RuntimeFactory {
    /// The read service the runner dispatches through.
    type Service: QueryOperations + ProfileSyncOperations;

    /// Builds the runtime, or reports a sanitized typed failure.
    fn build(&mut self) -> Result<ReadRuntime<Self::Service>, CliError>;
}

/// A [`RuntimeFactory`] over one set of [`ReadFoundations`].
///
/// The composition itself lives in [`compose`]; this type only maps the
/// sanitized composition failure into the CLI's typed error contract.
pub struct ComposingRuntimeFactory<F: ReadFoundations> {
    foundations: F,
}

impl<F: ReadFoundations> ComposingRuntimeFactory<F> {
    /// Wraps one set of foundation acquisitions.
    pub fn new(foundations: F) -> Self {
        Self { foundations }
    }
}

impl<F: ReadFoundations> RuntimeFactory for ComposingRuntimeFactory<F> {
    type Service = ComposedReadService<F>;

    fn build(&mut self) -> Result<ReadRuntime<Self::Service>, CliError> {
        compose(&mut self.foundations).map_err(CliError::from)
    }
}

/// The production factory the binary uses.
///
/// Constructing it performs no acquisition: the environment, credential store,
/// clock, transport, and database are only read when the boundary calls
/// [`RuntimeFactory::build`], after the request is already valid.
pub fn production_runtime_factory() -> ComposingRuntimeFactory<ProductionFoundations> {
    ComposingRuntimeFactory::new(ProductionFoundations)
}

/// Builds the production read runtime in one exact fail-fast order.
///
/// See the module documentation for the order and for what each failure
/// category means. The returned [`ReadFreshness`] carries the configured
/// policy and the single clock reading the composition performed.
pub fn compose<F>(foundations: &mut F) -> Result<ReadRuntime<ComposedReadService<F>>, AppError>
where
    F: ReadFoundations,
{
    let config = foundations.config()?;
    compose_configured(foundations, &config)
}

fn compose_configured<F: ReadFoundations>(
    foundations: &mut F,
    config: &ReadRuntimeConfig,
) -> Result<ReadRuntime<ComposedReadService<F>>, AppError> {
    let source = foundations.credential_source(config)?;
    let clock = foundations.clock()?;
    // Exactly one reading supplies the freshness evaluation time; the shared
    // services own the skip/refresh decision against it.
    let now = clock.now()?;
    let transport = foundations.transport()?;
    let api_key = ProfileApiKey::new(config.api_key().expose_secret())?;
    let repository = foundations.store(config)?;
    let version = config.resolve_definition_version(&repository)?;
    let gateway = BungieProfileGateway::new(transport, clock, source, api_key, version)?;
    let service = QueryService::new(ProfileSyncService::new_with_definitions(
        gateway, repository,
    ));
    let freshness = ReadFreshness {
        policy: config.freshness_policy(),
        now,
    };
    Ok(ReadRuntime::new(service, freshness))
}

/// Manifest acquisitions; weekly reset additionally requires a stored session.
///
/// Reuses shared read config (API key, database path, optional label, TTL),
/// which loads no OAuth-only key. Sync uses only the API key and database path;
/// it always publishes the real upstream version, never the optional override.
pub trait ManifestFoundations {
    /// Only authenticated weekly reads call this seam, never Manifest sync.
    fn require_session(&mut self, _config: &ReadRuntimeConfig) -> Result<(), AppError> {
        Err(AppError::new(ErrorCategory::AuthorizationRequired))
    }
    type Transport: destiny_adapters::manifest::ManifestTransport;
    type Clock: Clock;
    type Repository: destiny_app::DefinitionCatalogRepository
        + destiny_app::WorldDefinitionRepository
        + destiny_app::StatDefinitionRepository
        + destiny_app::ClarityRepository;

    /// Injectable, credential-free acquisition; legacy fixtures remain offline.
    fn clarity_transport(
        &mut self,
    ) -> Result<Box<dyn destiny_adapters::clarity::ClarityTransport>, AppError> {
        Err(AppError::new(
            destiny_app::ErrorCategory::ProviderUnavailable,
        ))
    }

    fn config(&mut self) -> Result<ReadRuntimeConfig, AppError>;
    fn clock(&mut self) -> Result<Self::Clock, AppError>;
    fn transport(&mut self) -> Result<Self::Transport, AppError>;
    fn store(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Repository, AppError>;
}

impl ManifestFoundations for ProductionFoundations {
    fn require_session(&mut self, config: &ReadRuntimeConfig) -> Result<(), AppError> {
        ReadFoundations::credential_source(self, config).map(|_| ())
    }
    fn clarity_transport(
        &mut self,
    ) -> Result<Box<dyn destiny_adapters::clarity::ClarityTransport>, AppError> {
        Ok(Box::new(
            destiny_adapters::clarity::HttpClarityTransport::new()?,
        ))
    }

    type Transport = destiny_adapters::manifest::HttpManifestTransport;
    type Clock = SystemClock;
    type Repository = SqliteStore;

    fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
        ReadRuntimeConfig::from_env()
    }
    fn clock(&mut self) -> Result<Self::Clock, AppError> {
        Ok(SystemClock::new())
    }
    fn transport(&mut self) -> Result<Self::Transport, AppError> {
        destiny_adapters::manifest::HttpManifestTransport::new()
    }
    fn store(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Repository, AppError> {
        SqliteStore::open(config.database_path())
    }
}

/// Runs one public Manifest sync; construction acquires nothing.
pub trait ManifestFactory {
    fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError>;

    fn sync_with_clarity(&mut self) -> Result<destiny_app::ManifestAndClarityReport, CliError> {
        Ok(destiny_app::ManifestAndClarityReport {
            manifest: self.sync()?,
            clarity: destiny_app::ClaritySyncReport::unavailable(),
        })
    }

    /// Opt-in preflight: injected legacy factories perform no new acquisitions.
    fn definitions_missing(&mut self) -> Result<bool, CliError> {
        Ok(false)
    }
}

pub struct ComposingManifestFactory<F> {
    foundations: F,
    automatic: bool,
}
impl<F> ComposingManifestFactory<F> {
    pub fn new(foundations: F) -> Self {
        Self {
            foundations,
            automatic: false,
        }
    }

    /// Enables first-use synchronization at the production process boundary.
    pub fn automatic(foundations: F) -> Self {
        Self {
            foundations,
            automatic: true,
        }
    }
}
impl<F: ManifestFoundations> ManifestFactory for ComposingManifestFactory<F> {
    fn definitions_missing(&mut self) -> Result<bool, CliError> {
        if !self.automatic {
            return Ok(false);
        }
        let config = self.foundations.config()?;
        let repository = self.foundations.store(&config)?;
        Ok(repository.current_metadata()?.is_none())
    }

    fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
        let report = self.sync_with_clarity()?;
        eprintln!("clarity: {}", report.clarity.status);
        Ok(report.manifest)
    }

    fn sync_with_clarity(&mut self) -> Result<destiny_app::ManifestAndClarityReport, CliError> {
        let foundations = &mut self.foundations;
        let config = foundations.config()?;
        let clock = foundations.clock()?;
        let checked_at = clock.now()?;
        let transport = foundations.transport()?;
        let api_key =
            destiny_adapters::manifest::ManifestApiKey::new(config.api_key().expose_secret())?;
        let gateway =
            destiny_adapters::manifest::BungieManifestGateway::new(api_key, transport, clock);
        let repository = foundations.store(&config)?;
        destiny_app::clarity::sync_manifest_and_clarity(
            &mut destiny_app::ManifestService::new(gateway, repository),
            || {
                foundations
                    .clarity_transport()
                    .map(destiny_adapters::clarity::ClarityAdapter)
            },
            checked_at,
        )
        .map_err(CliError::from)
    }
}

/// Authenticated weekly reset composition over the public milestones gateway.
pub trait WeeklyResetFactory {
    fn weekly_reset(
        &mut self,
        requested: Option<destiny_app::Timestamp>,
    ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError>;
}

impl<F: ManifestFoundations> WeeklyResetFactory for ComposingManifestFactory<F> {
    fn weekly_reset(
        &mut self,
        requested: Option<destiny_app::Timestamp>,
    ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
        let foundations = &mut self.foundations;
        let config = foundations.config()?;
        foundations.require_session(&config)?;
        let clock = foundations.clock()?;
        let now = clock.now()?;
        let calendar = destiny_adapters::calendar::embedded_calendar()
            .map_err(|_| CliError::from_category(ErrorCategory::ClockOverflow))?;
        let transport = foundations.transport()?;
        let api_key =
            destiny_adapters::manifest::ManifestApiKey::new(config.api_key().expose_secret())?;
        let gateway =
            destiny_adapters::manifest::BungieManifestGateway::new(api_key, transport, clock);
        let repository = foundations.store(&config)?;
        destiny_app::weekly_reset::WeeklyResetService::new(gateway, repository)
            .weekly_reset(&calendar, requested.unwrap_or(now), now)
            .map_err(CliError::from)
    }
}

/// Exact IDs take precedence over case-insensitive class selectors.
/// Default to unique highest light; ties/missing light retain canonical first.
pub fn select_weekly_character(
    characters: &[destiny_app::Character],
    selector: Option<&destiny_app::CharacterId>,
) -> Result<destiny_app::Character, CliError> {
    destiny_app::weekly_selection::select_weekly_character(characters, selector).map_err(|error| {
        match error {
            destiny_app::weekly_selection::WeeklySelectionError::InvalidRequest => {
                CliError::invalid_request()
            }
            destiny_app::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                CliError::from_category(ErrorCategory::AmbiguousMatch)
            }
        }
    })
}

/// Private handles to ONE invocation-owned connection. No handles escape this
/// synchronous composition, so no caller-local publisher can interleave with
/// planning, definition use, preflight, dispatch or observations. This does not
/// serialize another process, the game, or any remote client.
#[derive(Clone)]
struct PullStore(std::rc::Rc<std::cell::RefCell<SqliteStore>>);

#[cfg(test)]
mod pull_store_tests {
    use super::PullStore;
    use destiny_adapters::sqlite::SqliteStore;
    use destiny_app::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn pull_store_one_connection_shares_definition_and_inventory_publications() {
        let mut inventory = PullStore(Rc::new(RefCell::new(
            SqliteStore::open_in_memory().unwrap(),
        )));
        let mut definitions = inventory.clone();
        assert!(Rc::ptr_eq(&inventory.0, &definitions.0));
        let version = DefinitionVersion::new("pull-store-test");
        let definition = ItemDefinition {
            hash: ItemHash::new(7),
            bucket_hash: Some(BucketHash::new(1498876634)),
            does_postmaster_pull_have_side_effects: Some(false),
            is_instance_item: Some(true),
            max_stack_size: None,
            stack_unique_label: None,
            name: "Offline equipment".into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: version.clone(),
        };
        let metadata = definitions
            .replace_definitions(DefinitionReplacementInput {
                definitions: ManifestDefinitions {
                    version: version.clone(),
                    definitions: vec![definition.clone()],
                },
                checked_at: Timestamp::new(100),
                source: SourceMarker::Manifest,
            })
            .unwrap();
        assert_eq!(inventory.current_definition_version().unwrap(), version);
        assert_eq!(
            inventory.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(definition)
        );
        assert_eq!(inventory.current_metadata().unwrap(), Some(metadata));
        let input = InventorySnapshotInput {
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: version,
            checked_at: Timestamp::new(100),
            source: SourceMarker::Profile,
            characters: vec![],
            items: vec![],
            stacks: vec![],
            plug_set_entries: vec![],
            profile_currencies_coverage: Default::default(),
        };
        let minted = destiny_app::ProfileMintedAt::new(100, 123456789).unwrap();
        let first = inventory
            .replace_current_minted(input.clone(), Some(minted))
            .unwrap();
        assert_eq!(definitions.profile_minted_at().unwrap(), Some(minted));
        assert_eq!(definitions.load_current().unwrap(), Some(first.clone()));
        let mut refreshed = input;
        refreshed.checked_at = Timestamp::new(101);
        let second = definitions.replace_current(refreshed).unwrap();
        assert_eq!(inventory.profile_minted_at().unwrap(), None);
        assert_eq!(second.metadata.revision, first.metadata.revision);
        assert_eq!(inventory.load_current().unwrap(), Some(second));
    }
}
impl InventoryRepository for PullStore {
    fn saved_loadouts(&self) -> Result<Option<Vec<destiny_app::CharacterLoadouts>>, AppError> {
        self.0.borrow().saved_loadouts()
    }
    fn replace_current_profile(
        &mut self,
        input: destiny_app::InventorySnapshotInput,
        minted: Option<destiny_app::ProfileMintedAt>,
        loadouts: Option<Vec<destiny_app::CharacterLoadouts>>,
    ) -> Result<destiny_app::InventorySnapshot, AppError> {
        self.0
            .borrow_mut()
            .replace_current_profile(input, minted, loadouts)
    }
    fn profile_minted_at(&self) -> Result<Option<destiny_app::ProfileMintedAt>, AppError> {
        self.0.borrow().profile_minted_at()
    }
    fn replace_current_minted(
        &mut self,
        input: destiny_app::InventorySnapshotInput,
        minted: Option<destiny_app::ProfileMintedAt>,
    ) -> Result<destiny_app::InventorySnapshot, AppError> {
        self.0.borrow_mut().replace_current_minted(input, minted)
    }
    fn freshness_invalidated(&self) -> Result<bool, AppError> {
        self.0.borrow().freshness_invalidated()
    }
    fn invalidate_freshness(&mut self) -> Result<(), AppError> {
        self.0.borrow_mut().invalidate_freshness()
    }
    fn apply_acknowledged_effects(
        &mut self,
        membership: &destiny_app::DestinyMembership,
        effects: &[destiny_app::InventoryEffect],
    ) -> Result<destiny_app::InventorySnapshot, AppError> {
        self.0
            .borrow_mut()
            .apply_acknowledged_effects(membership, effects)
    }
    fn load_current(&self) -> Result<Option<destiny_app::InventorySnapshot>, AppError> {
        self.0.borrow().load_current()
    }
    fn replace_current(
        &mut self,
        input: destiny_app::InventorySnapshotInput,
    ) -> Result<destiny_app::InventorySnapshot, AppError> {
        self.0.borrow_mut().replace_current(input)
    }
}
impl DefinitionRepository for PullStore {
    fn current_definition_version(&self) -> Result<destiny_app::DefinitionVersion, AppError> {
        self.0.borrow().current_definition_version()
    }
    fn lookup_definition(
        &self,
        hash: destiny_app::ItemHash,
    ) -> Result<destiny_app::Resolution<destiny_app::ItemDefinition>, AppError> {
        self.0.borrow().lookup_definition(hash)
    }
    fn current_metadata(&self) -> Result<Option<destiny_app::SyncMetadata>, AppError> {
        DefinitionRepository::current_metadata(&*self.0.borrow())
    }
    fn replace_definitions(
        &mut self,
        input: destiny_app::DefinitionReplacementInput,
    ) -> Result<destiny_app::SyncMetadata, AppError> {
        self.0.borrow_mut().replace_definitions(input)
    }
}

/// Authenticated membership and fresh Postmaster ownership, never CLI overrides.
/// One store, fresh concrete Manifest qualification, and one consumed plan. No
/// automatic definition download/retry after failure or received action outcome.
pub fn compose_pull(
    item: destiny_app::ItemInstanceId,
) -> Result<crate::pull_output::PullOutput, CliError> {
    use destiny_adapters::instanced_pull::{
        BungieInstancedPullGateway, HttpInstancedPullTransport, InstancedPullApiKey,
    };
    use destiny_adapters::manifest::{
        BungieManifestGateway, HttpManifestTransport, ManifestApiKey,
    };
    use destiny_app::ProfileGateway;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = PullStore(std::rc::Rc::new(std::cell::RefCell::new(
        ReadFoundations::store(&mut foundations, &config)?,
    )));
    let mut profile = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let membership =
        profile.resolve_membership_scoped(destiny_app::ProfileSyncReason::Preflight)?;
    let manifest = BungieManifestGateway::new(
        ManifestApiKey::new(config.api_key().expose_secret())?,
        HttpManifestTransport::new()?,
        SystemClock::new(),
    );
    let mut gateway = BungieInstancedPullGateway::new(
        HttpInstancedPullTransport::new()?,
        SystemClock::new(),
        source,
        InstancedPullApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut service = destiny_app::InstancedPullService::new(
        ProfileSyncService::new_with_definitions(profile, repository.clone()),
        repository,
        manifest,
        destiny_adapters::pinned_equipment_capacity_policy(),
        SystemClock::new(),
        crate::pull::InvocationPlanId::new(),
    );
    Ok(crate::pull::pull_item(
        item,
        membership,
        &mut service,
        &mut gateway,
        config.freshness_policy(),
    ))
}

/// One invocation-owned connection, authenticated membership and fresh Manifest
/// capacity evidence. No recompose, automatic download or action retry.
pub fn compose_pull_stack(
    selection: crate::command::StackPullSelection,
) -> Result<crate::pull_stack_output::StackPullOutput, CliError> {
    use destiny_adapters::manifest::{
        BungieManifestGateway, HttpManifestTransport, ManifestApiKey,
    };
    use destiny_adapters::stack_pull::{
        BungieStackPullGateway, HttpStackPullTransport, StackPullApiKey,
    };
    use destiny_app::ProfileGateway;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = PullStore(std::rc::Rc::new(std::cell::RefCell::new(
        ReadFoundations::store(&mut foundations, &config)?,
    )));
    let mut profile = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let membership =
        profile.resolve_membership_scoped(destiny_app::ProfileSyncReason::Preflight)?;
    let manifest = BungieManifestGateway::new(
        ManifestApiKey::new(config.api_key().expose_secret())?,
        HttpManifestTransport::new()?,
        SystemClock::new(),
    );
    let mut gateway = BungieStackPullGateway::new(
        HttpStackPullTransport::new()?,
        SystemClock::new(),
        source,
        StackPullApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut service = destiny_app::StackPullService::new(
        ProfileSyncService::new_with_definitions(profile, repository.clone()),
        repository,
        manifest,
        destiny_adapters::stack_bucket_capacity_policy::pinned_stack_bucket_capacity_policy(),
        SystemClock::new(),
        crate::pull::InvocationPlanId::new(),
    );
    Ok(crate::pull_stack::pull_stack(
        selection,
        membership,
        &mut service,
        &mut gateway,
        config.freshness_policy(),
    ))
}

/// Compose all three gateways over one lock-backed credential session, so a
/// confidential refresh cannot race or rotate independently between actions.
pub fn compose_equip(
    arguments: &crate::command::EquipArgs,
) -> Result<crate::equip_output::EquipOutput, CliError> {
    use destiny_adapters::equip::{BungieEquipGateway, EquipApiKey, HttpEquipTransport};
    use destiny_adapters::transfer::{
        BungieTransferGateway, HttpTransferTransport, TransferApiKey,
    };
    arguments.validate()?;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = ReadFoundations::store(&mut foundations, &config)?;
    let gateway = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let mut transfer = BungieTransferGateway::new(
        HttpTransferTransport::new()?,
        SystemClock::new(),
        source.clone(),
        TransferApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut equip = BungieEquipGateway::new(
        HttpEquipTransport::new()?,
        SystemClock::new(),
        source,
        EquipApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut mutations = destiny_app::MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
        crate::equip::InvocationPlanId::new(),
    );
    crate::equip::equip_item(
        arguments,
        &mut mutations,
        &mut transfer,
        &mut equip,
        &SystemClock::new(),
        config.freshness_policy(),
    )
}

/// Compose one combined batch using existing session credentials and per-action clients.
pub fn compose_equip_items(
    arguments: &crate::command::EquipItemsArgs,
) -> Result<destiny_output::equip_items::EquipItemsOutput, CliError> {
    let request = arguments.request()?;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = ReadFoundations::store(&mut foundations, &config)?;
    let gateway = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let mut transfer = destiny_adapters::BungieTransferGateway::new(
        destiny_adapters::HttpTransferTransport::new()?,
        SystemClock::new(),
        source.clone(),
        destiny_adapters::TransferApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut action = destiny_adapters::BungieEquipItemsGateway::new(
        destiny_adapters::HttpEquipItemsTransport::new()?,
        SystemClock::new(),
        source,
        destiny_adapters::EquipItemsApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut service = destiny_app::MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
        crate::equip::InvocationPlanId::new(),
    );
    let mut last = None;
    let result = service.equip_items(
        (&mut transfer, &mut action),
        &request,
        config.freshness_policy(),
        SystemClock::new().now()?,
        || destiny_app::equip_items::pace(&mut last),
    );
    Ok(destiny_output::equip_items::EquipItemsOutput::from_result(
        result,
    ))
}

/// Compose one saved-loadout action over the same lock-backed session as profile.
pub fn compose_apply_loadout(
    arguments: &crate::command::ApplyLoadoutArgs,
) -> Result<destiny_output::apply_loadout::ApplyLoadoutOutput, CliError> {
    use destiny_adapters::loadout_action::{
        BungieLoadoutActionGateway, HttpLoadoutActionTransport, LoadoutActionApiKey,
    };
    let request = arguments.request()?;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let repository = ReadFoundations::store(&mut foundations, &config)?;
    let gateway = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        SystemClock::new(),
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let mut action = BungieLoadoutActionGateway::new(
        HttpLoadoutActionTransport::new()?,
        SystemClock::new(),
        source,
        LoadoutActionApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut service = destiny_app::ApplyLoadoutService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
    );
    Ok(
        destiny_output::apply_loadout::ApplyLoadoutOutput::from_result(service.apply(
            &mut action,
            &request,
            config.freshness_policy(),
            SystemClock::new().now()?,
        )),
    )
}

/// Compose profile and lock gateways over one credential session and one local
/// repository. The synchronous invocation owns publication through dispatch.
pub fn compose_lock(
    arguments: &crate::command::LockArgs,
    locked: bool,
) -> Result<crate::lock_output::LockOutput, CliError> {
    use destiny_adapters::lock::{BungieLockGateway, HttpLockTransport, LockApiKey};
    arguments.validate()?;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = ReadFoundations::store(&mut foundations, &config)?;
    let gateway = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let mut lock = BungieLockGateway::new(
        HttpLockTransport::new()?,
        SystemClock::new(),
        source,
        LockApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut mutations = destiny_app::MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
        crate::equip::InvocationPlanId::new(),
    );
    crate::lock::set_lock_state(
        arguments,
        locked,
        &mut mutations,
        &mut lock,
        &SystemClock::new(),
        config.freshness_policy(),
    )
}

/// Compose profile and transfer gateways over one lock-backed credential session.
pub fn compose_move(
    arguments: &crate::command::MoveArgs,
) -> Result<crate::move_output::MoveOutput, CliError> {
    use destiny_adapters::transfer::{
        BungieTransferGateway, HttpTransferTransport, TransferApiKey,
    };
    arguments.validate()?;
    let mut foundations = ProductionFoundations;
    let config = ReadFoundations::config(&mut foundations)?;
    let source = foundations.credential_source(&config)?;
    let clock = ReadFoundations::clock(&mut foundations)?;
    let repository = ReadFoundations::store(&mut foundations, &config)?;
    let gateway = BungieProfileGateway::new(
        ReadFoundations::transport(&mut foundations)?,
        clock,
        source.clone(),
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&repository)?,
    )?;
    let mut transfer = BungieTransferGateway::new(
        HttpTransferTransport::new()?,
        SystemClock::new(),
        source,
        TransferApiKey::new(config.api_key().expose_secret())?,
    )?;
    let mut mutations = destiny_app::MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, repository),
        crate::move_item::InvocationPlanId::new(),
    );
    crate::move_item::move_item(
        arguments,
        &mut mutations,
        &mut transfer,
        &SystemClock::new(),
        config.freshness_policy(),
    )
}

/// Composes the live artifact query with the real authenticated profile gateway.
/// Freshness synchronization and definition gating remain application-owned.
pub fn compose_artifact<F: ReadFoundations>(
    foundations: &mut F,
    selector: Option<&destiny_app::CharacterId>,
) -> Result<destiny_app::ArtifactResult, CliError> {
    let config = foundations.config()?;
    let runtime = compose_configured(foundations, &config)?;
    destiny_app::ArtifactService::new(runtime.service.into_sync())
        .artifact(selector, runtime.freshness.policy, runtime.freshness.now)
        .map_err(CliError::from)
}

/// Composes the live loadout query over the authenticated, synchronized projection.
pub fn compose_loadouts<F>(
    foundations: &mut F,
    selector: Option<&destiny_app::CharacterId>,
) -> Result<destiny_app::LoadoutsResult, CliError>
where
    F: ReadFoundations,
    F::Repository: destiny_app::WorldDefinitionRepository,
{
    let config = foundations.config()?;
    let runtime = compose_configured(foundations, &config)?;
    destiny_app::LoadoutService::new(runtime.service.into_sync())
        .loadouts(selector, runtime.freshness.policy, runtime.freshness.now)
        .map_err(CliError::from)
}

/// Reuses authenticated read composition, including its configured refresh source.
/// Definitions gate precedes even membership/inventory requests.
pub fn compose_weekly<F, T>(
    foundations: &mut F,
    public_transport: T,
    selector: Option<&destiny_app::CharacterId>,
) -> Result<
    (
        destiny_app::Character,
        destiny_app::weekly_personal::WeeklyPersonal,
    ),
    CliError,
>
where
    F: ReadFoundations,
    F::Repository: destiny_app::WorldDefinitionRepository,
    T: destiny_adapters::manifest::ManifestTransport,
{
    use destiny_app::{DestinyMembership, WorldDefinitionRepository};
    let config = foundations.config()?;
    let runtime = compose_configured(foundations, &config)?;
    let freshness = runtime.freshness;
    let mut service = runtime.service;
    let repository = service.repository();
    if repository.current_metadata()?.is_none()
        || repository.world_definition_counts()?.contains(&0)
    {
        return Err(CliError::from_category(
            ErrorCategory::DefinitionsUnavailable,
        ));
    }
    let characters = service.list_characters(freshness.policy, freshness.now)?;
    let character = select_weekly_character(&characters.characters, selector)?;
    let snapshot = service
        .repository()
        .load_current()?
        .ok_or_else(|| CliError::from_category(ErrorCategory::LocalStorage))?;
    let membership = DestinyMembership::new(
        snapshot.metadata.membership_type,
        snapshot.metadata.membership_id,
    )?;
    let (gateway, repository) = service.into_sync().into_parts();
    let public_key =
        destiny_adapters::manifest::ManifestApiKey::new(config.api_key().expose_secret())?;
    let public = destiny_adapters::manifest::BungieManifestGateway::new(
        public_key,
        public_transport,
        destiny_adapters::SystemClock::new(),
    );
    let weekly =
        destiny_app::weekly_personal::WeeklyPersonalService::new(public, gateway, repository)
            .weekly(&destiny_app::weekly_personal::WeeklyCharacter {
                membership,
                character_id: character.id.clone(),
            })?;
    Ok((character, weekly))
}

#[cfg(test)]
mod tests {
    use super::{
        ComposingRuntimeFactory, ProductionFoundations, ReadFoundations, RuntimeFactory, compose,
        load_required_credential,
    };
    use crate::command::{CliError, ReadRequest};
    use crate::output::OutputMode;
    use crate::runner::{ReadFreshness, dispatch};
    use destiny_adapters::profile_gateway::{
        ProfileHttpRequest, ProfileHttpResponse, ProfileTransport, ProfileTransportError,
    };
    use destiny_adapters::sqlite::SqliteStore;
    use destiny_app::{
        AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial,
        CredentialStore, ErrorCategory, FreshnessPolicy, ReadRuntimeConfig, Timestamp,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(1_700_000_000);
    const TTL_SECONDS: u64 = 900;

    /// Canonical sentinels that no diagnostic may ever render.
    const API_KEY_SENTINEL: &str = "sentinel-runtime-api-key";
    const DATABASE_SENTINEL: &str = "/sentinel/runtime/destiny.db";
    const BEARER_SENTINEL: &str = "sentinel-runtime-bearer";

    /// A clock with a scripted reading that records each `now` call.
    #[derive(Clone, Debug)]
    struct ScriptedClock {
        now: Result<Timestamp, AppError>,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Clock for ScriptedClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            self.log.borrow_mut().push("now");
            self.now.clone()
        }
    }

    /// A transport that is never reached by the composition tests.
    #[derive(Clone, Copy, Debug, Default)]
    struct UnreachableTransport;

    impl ProfileTransport for UnreachableTransport {
        fn get(
            &mut self,
            _request: &ProfileHttpRequest<'_>,
        ) -> Result<ProfileHttpResponse, ProfileTransportError> {
            Err(ProfileTransportError::Unavailable)
        }
    }

    /// Scripted foundations that record the exact acquisition order.
    struct FakeFoundations {
        log: Rc<RefCell<Vec<&'static str>>>,
        config: Result<ReadRuntimeConfig, AppError>,
        credential: Result<AccessCredential, AppError>,
        clock: Result<ScriptedClock, AppError>,
        transport: Result<UnreachableTransport, AppError>,
        store: Option<Result<SqliteStore, AppError>>,
    }

    impl FakeFoundations {
        /// Foundations whose only failure is the durable store, so every step
        /// before it succeeds and is observable in the log.
        fn store_failure(log: Rc<RefCell<Vec<&'static str>>>) -> Self {
            Self {
                clock: Ok(ScriptedClock {
                    now: Ok(NOW),
                    log: Rc::clone(&log),
                }),
                config: Ok(config()),
                credential: Ok(credential()),
                transport: Ok(UnreachableTransport),
                store: Some(Err(AppError::new(ErrorCategory::LocalStorage))),
                log,
            }
        }

        fn record(&self, step: &'static str) {
            self.log.borrow_mut().push(step);
        }
    }

    impl ReadFoundations for FakeFoundations {
        type Source = destiny_app::ConfiguredCredentialSource<
            FakeCredentialStore,
            FakeRefresh,
            ScriptedClock,
        >;
        type Transport = UnreachableTransport;
        type Clock = ScriptedClock;
        type Repository = SqliteStore;

        fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
            self.record("config");
            self.config.clone()
        }

        fn credential_source(
            &mut self,
            config: &ReadRuntimeConfig,
        ) -> Result<Self::Source, AppError> {
            self.record("credential");
            super::compose_credentials(
                config,
                FakeCredentialStore {
                    result: self.credential.clone().map(Some),
                },
                |_| {
                    self.record("refreshing");
                    Ok(FakeRefresh)
                },
                ScriptedClock {
                    now: Ok(NOW),
                    log: Rc::clone(&self.log),
                },
            )
        }

        fn clock(&mut self) -> Result<ScriptedClock, AppError> {
            self.record("clock");
            self.clock.clone()
        }

        fn transport(&mut self) -> Result<UnreachableTransport, AppError> {
            self.record("transport");
            self.transport.clone()
        }

        fn store(&mut self, _config: &ReadRuntimeConfig) -> Result<SqliteStore, AppError> {
            self.record("store");
            self.store.take().expect("the store step runs once")
        }
    }

    fn config() -> ReadRuntimeConfig {
        ReadRuntimeConfig::from_values(&HashMap::from([
            ("BUNGIE_API_KEY".to_string(), API_KEY_SENTINEL.to_string()),
            (
                "DESTINY_DATABASE_PATH".to_string(),
                DATABASE_SENTINEL.to_string(),
            ),
            (
                "DESTINY_DEFINITION_VERSION".to_string(),
                "manifest-v1".to_string(),
            ),
            (
                "DESTINY_FRESHNESS_TTL_SECONDS".to_string(),
                TTL_SECONDS.to_string(),
            ),
        ]))
        .expect("valid test configuration")
    }

    fn credential() -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(4_000_000_000),
            },
            material: CredentialMaterial::from_bytes(BEARER_SENTINEL.as_bytes().to_vec()),
            refresh_grant: None,
        }
    }

    fn log() -> Rc<RefCell<Vec<&'static str>>> {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// A credential store fake returning one scripted load outcome.
    #[derive(Clone)]
    struct FakeCredentialStore {
        result: Result<Option<AccessCredential>, AppError>,
    }

    impl CredentialStore for FakeCredentialStore {
        fn save(&mut self, _credential: AccessCredential) -> Result<(), AppError> {
            Ok(())
        }

        fn load(&self) -> Result<Option<AccessCredential>, AppError> {
            self.result.clone()
        }

        fn delete(&mut self) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct FakeRefresh;
    impl destiny_app::TokenRefresh for FakeRefresh {
        fn refresh(&mut self, _: &destiny_app::RefreshToken) -> Result<AccessCredential, AppError> {
            Ok(credential())
        }
    }

    /// One scripted foundation failure and its expected stable CLI outcome.
    type FailureCase = (&'static str, fn(&mut FakeFoundations), &'static str, i32);

    fn assert_failure_is_sanitized(error: CliError, exit_code: i32, code: &str) {
        assert_eq!(error.exit_code(), exit_code);
        assert_eq!(error.code(), code);
        let rendered = format!("{error} {error:?}");
        for sentinel in [API_KEY_SENTINEL, DATABASE_SENTINEL, BEARER_SENTINEL] {
            assert!(
                !rendered.contains(sentinel),
                "a diagnostic leaked {sentinel}: {rendered}"
            );
        }
    }

    struct FakeManifestFoundations {
        log: Rc<RefCell<Vec<&'static str>>>,
        store: Option<SqliteStore>,
    }

    struct ManifestTransport;
    impl destiny_adapters::manifest::ManifestTransport for ManifestTransport {
        fn get(
            &mut self,
            request: &destiny_adapters::manifest::ManifestHttpRequest<'_>,
        ) -> Result<
            destiny_adapters::manifest::ManifestHttpResponse,
            destiny_adapters::manifest::ManifestTransportError,
        > {
            let body = if request.url().contains("/Platform/") {
                r#"{"ErrorCode":1,"Response":{"version":"v1","jsonWorldComponentContentPaths":{"en":{"DestinyInventoryItemDefinition":"/common/destiny2_content/items.json","DestinyMilestoneDefinition":"/common/destiny2_content/m.json","DestinyActivityDefinition":"/common/destiny2_content/a.json","DestinyActivityModifierDefinition":"/common/destiny2_content/mod.json","DestinyObjectiveDefinition":"/common/destiny2_content/obj.json","DestinyActivityTypeDefinition":"/common/destiny2_content/type.json","DestinyStatDefinition":"/common/destiny2_content/stat.json","DestinyLoadoutNameDefinition":"/common/destiny2_content/loadout-names.json","DestinySandboxPerkDefinition":"/common/destiny2_content/sandbox-perks.json"}}}}"#
            } else if request.url().ends_with("/sandbox-perks.json") {
                "{}"
            } else if request.url().ends_with("/loadout-names.json") {
                r#"{"1":{"hash":1,"name":"PvE"}}"#
            } else if request.url().ends_with("/obj.json") {
                r#"{"1":{"hash":1,"progressDescription":"","completionValue":1},"2":{"hash":2,"redacted":true}}"#
            } else {
                r#"{"1":{"hash":1,"displayProperties":{"name":"Item"}},"2":{"hash":2,"displayProperties":{"name":""}}}"#
            };
            Ok(destiny_adapters::manifest::ManifestHttpResponse::new(
                200,
                None,
                Some("application/json".into()),
                Box::new(std::io::Cursor::new(body.as_bytes().to_vec())),
            ))
        }
    }

    struct FailingClarityTransport;
    impl destiny_adapters::clarity::ClarityTransport for FailingClarityTransport {
        fn get(
            &mut self,
            resource: destiny_adapters::clarity::ClarityResource,
        ) -> Result<Vec<u8>, AppError> {
            assert_eq!(
                resource,
                destiny_adapters::clarity::ClarityResource::Versions
            );
            Err(AppError::new(ErrorCategory::ProviderUnavailable))
        }
    }

    impl super::ManifestFoundations for FakeManifestFoundations {
        fn clarity_transport(
            &mut self,
        ) -> Result<Box<dyn destiny_adapters::clarity::ClarityTransport>, AppError> {
            Ok(Box::new(FailingClarityTransport))
        }
        type Transport = ManifestTransport;
        type Clock = ScriptedClock;
        type Repository = SqliteStore;
        fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
            self.log.borrow_mut().push("config");
            Ok(config())
        }
        fn clock(&mut self) -> Result<Self::Clock, AppError> {
            self.log.borrow_mut().push("clock");
            Ok(ScriptedClock {
                now: Ok(NOW),
                log: Rc::clone(&self.log),
            })
        }
        fn transport(&mut self) -> Result<Self::Transport, AppError> {
            self.log.borrow_mut().push("transport");
            Ok(ManifestTransport)
        }
        fn store(&mut self, _: &ReadRuntimeConfig) -> Result<Self::Repository, AppError> {
            self.log.borrow_mut().push("store");
            Ok(self.store.take().unwrap())
        }
    }

    struct WeeklyTransport {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }
    impl destiny_adapters::manifest::ManifestTransport for WeeklyTransport {
        fn get(
            &mut self,
            request: &destiny_adapters::manifest::ManifestHttpRequest<'_>,
        ) -> Result<
            destiny_adapters::manifest::ManifestHttpResponse,
            destiny_adapters::manifest::ManifestTransportError,
        > {
            assert_eq!(
                request.url(),
                "https://www.bungie.net/Platform/Destiny2/Milestones/"
            );
            self.calls.borrow_mut().push("milestones");
            Ok(destiny_adapters::manifest::ManifestHttpResponse::new(200, None, Some("application/json".into()), Box::new(std::io::Cursor::new(br#"{"ErrorCode":1,"ErrorStatus":"Success","Response":{"1":{"milestoneHash":1,"order":1,"activities":[{"activityHash":1,"modifierHashes":[1]}]}}}"#.to_vec()))))
        }
    }
    struct WeeklyFoundations {
        inner: FakeManifestFoundations,
        missing_login: bool,
    }
    impl super::ManifestFoundations for WeeklyFoundations {
        fn require_session(&mut self, _: &ReadRuntimeConfig) -> Result<(), AppError> {
            let mut foundations = PersonalFoundations {
                store: None,
                calls: Rc::clone(&self.inner.log),
                missing_login: self.missing_login,
            };
            foundations.credential_source(&config()).map(|_| ())
        }
        type Transport = WeeklyTransport;
        type Clock = ScriptedClock;
        type Repository = SqliteStore;
        fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
            self.inner.config()
        }
        fn clock(&mut self) -> Result<Self::Clock, AppError> {
            Ok(ScriptedClock {
                now: Ok(
                    destiny_adapters::calendar::instant_for_date("2026-10-06", &chrono::Utc)
                        .unwrap(),
                ),
                log: Rc::clone(&self.inner.log),
            })
        }
        fn transport(&mut self) -> Result<Self::Transport, AppError> {
            Ok(WeeklyTransport {
                calls: Rc::clone(&self.inner.log),
            })
        }
        fn store(&mut self, config: &ReadRuntimeConfig) -> Result<Self::Repository, AppError> {
            self.inner.store(config)
        }
    }

    fn weekly_character(id: &str) -> destiny_app::Character {
        destiny_app::Character {
            id: destiny_app::CharacterId::new(id),
            name: None,
            class: Some("Hunter".into()),
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }
    }

    #[test]
    fn weekly_selection_missing_light_falls_back_and_unknown_ids_are_rejected() {
        use super::select_weekly_character;
        use destiny_app::CharacterId;
        let characters = vec![weekly_character("123"), weekly_character("456")];
        assert_eq!(
            select_weekly_character(&characters, None).unwrap(),
            characters[0]
        );
        assert_eq!(
            select_weekly_character(&characters, Some(&CharacterId::new("456"))).unwrap(),
            characters[1]
        );
        for id in [" 123", "unknown"] {
            assert_eq!(
                select_weekly_character(&characters, Some(&CharacterId::new(id)))
                    .unwrap_err()
                    .code(),
                "invalid_request"
            );
        }
        assert_eq!(
            select_weekly_character(&[], None).unwrap_err().code(),
            "invalid_request"
        );
        assert_eq!(
            select_weekly_character(
                &[characters[0].clone(), characters[0].clone()],
                Some(&CharacterId::new("123"))
            )
            .unwrap_err()
            .code(),
            "ambiguous_match"
        );
    }

    #[test]
    fn weekly_selection_errors_preserve_baseline_rendered_messages() {
        use crate::output::{OutputMode, render_error};
        let characters = vec![weekly_character("123"), weekly_character("456")];
        for selector in ["unknown", "", " ", "id\nvalue"] {
            let error = super::select_weekly_character(
                &characters,
                Some(&destiny_app::CharacterId::new(selector)),
            )
            .unwrap_err();
            assert_eq!(
                render_error(OutputMode::Human, &error),
                "error: the request arguments are invalid"
            );
            let json: serde_json::Value =
                serde_json::from_str(&render_error(OutputMode::Json, &error)).unwrap();
            assert_eq!(json["error"]["code"], "invalid_request");
            assert_eq!(
                json["error"]["message"],
                "the request arguments are invalid"
            );
        }
        for (characters, selector, code, message) in [
            (
                vec![],
                None,
                "invalid_request",
                "the request arguments are invalid",
            ),
            (
                characters,
                Some(destiny_app::CharacterId::new("hunter")),
                "ambiguous_match",
                "the request matched multiple items",
            ),
        ] {
            let error = super::select_weekly_character(&characters, selector.as_ref()).unwrap_err();
            assert_eq!(
                render_error(OutputMode::Human, &error),
                format!("error: {message}")
            );
            let json: serde_json::Value =
                serde_json::from_str(&render_error(OutputMode::Json, &error)).unwrap();
            assert_eq!(json["error"]["code"], code);
            assert_eq!(json["error"]["message"], message);
        }
    }

    #[test]
    fn weekly_default_uses_unique_highest_light_independent_of_order() {
        let mut lower = weekly_character("123");
        lower.light = Some(100);
        let mut highest = weekly_character("456");
        highest.light = Some(200);
        for characters in [
            vec![lower.clone(), highest.clone()],
            vec![highest.clone(), lower.clone()],
            vec![weekly_character("789"), highest.clone()],
        ] {
            assert_eq!(
                super::select_weekly_character(&characters, None).unwrap(),
                highest
            );
        }
        lower.light = Some(200);
        let characters = vec![weekly_character("789"), lower, highest];
        assert_eq!(
            super::select_weekly_character(&characters, None).unwrap(),
            characters[0]
        );
    }

    #[test]
    fn weekly_class_selector_is_case_insensitive_unique_and_id_first() {
        use destiny_app::CharacterId;
        let hunter = weekly_character("123");
        let mut titan = weekly_character("456");
        titan.class = Some("Titan".into());
        let mut warlock = weekly_character("789");
        warlock.class = Some("Warlock".into());
        let characters = vec![hunter.clone(), titan.clone(), warlock.clone()];
        for (selector, expected) in [
            ("hunter", &hunter),
            ("HuNtEr", &hunter),
            ("TITAN", &titan),
            ("warlock", &warlock),
            ("456", &titan),
        ] {
            assert_eq!(
                super::select_weekly_character(&characters, Some(&CharacterId::new(selector)))
                    .unwrap(),
                *expected
            );
        }
        let duplicate = vec![hunter.clone(), weekly_character("999")];
        assert_eq!(
            super::select_weekly_character(&duplicate, Some(&CharacterId::new("hunter")))
                .unwrap_err()
                .code(),
            "ambiguous_match"
        );
        let mut exact = titan;
        exact.id = CharacterId::new("hunter");
        assert_eq!(
            super::select_weekly_character(
                &[hunter, exact.clone()],
                Some(&CharacterId::new("hunter"))
            )
            .unwrap(),
            exact
        );
    }

    #[test]
    fn profile_sync_is_accepted_at_process_boundary() {
        assert!(crate::runner::parse_process(["ghostlink", "sync", "--json"]).is_ok());
    }

    struct SyncTransport {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail: bool,
    }
    impl ProfileTransport for SyncTransport {
        fn get(
            &mut self,
            request: &ProfileHttpRequest<'_>,
        ) -> Result<ProfileHttpResponse, ProfileTransportError> {
            let body = if request
                .url()
                .ends_with("User/GetMembershipsForCurrentUser/")
            {
                self.calls.borrow_mut().push("membership");
                br#"{"ErrorCode":1,"Response":{"primaryMembershipId":null,"destinyMemberships":[{"membershipId":"999","membershipType":3,"crossSaveOverride":0,"applicableMembershipTypes":[3]}]}}"#.to_vec()
            } else {
                assert!(request.url().contains("/Destiny2/3/Profile/999/"));
                self.calls.borrow_mut().push("profile");
                if self.fail {
                    return Ok(ProfileHttpResponse::new(
                        400,
                        None,
                        Some("application/json".into()),
                        Box::new(std::io::Cursor::new(b"sentinel-provider-body".to_vec())),
                    ));
                }
                include_bytes!("../../destiny-adapters/tests/fixtures/profile-complete.json")
                    .to_vec()
            };
            Ok(ProfileHttpResponse::new(
                200,
                None,
                Some("application/json".into()),
                Box::new(std::io::Cursor::new(body)),
            ))
        }
    }
    struct SyncFoundations {
        store: Option<SqliteStore>,
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail: bool,
    }
    impl ReadFoundations for SyncFoundations {
        type Source = destiny_app::StaticCredentialSource;
        type Transport = SyncTransport;
        type Clock = ScriptedClock;
        type Repository = SqliteStore;
        fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
            Ok(config())
        }
        fn credential_source(&mut self, _: &ReadRuntimeConfig) -> Result<Self::Source, AppError> {
            Ok(destiny_app::StaticCredentialSource::new(credential()))
        }
        fn clock(&mut self) -> Result<Self::Clock, AppError> {
            Ok(ScriptedClock {
                now: Ok(NOW),
                log: Rc::clone(&self.calls),
            })
        }
        fn transport(&mut self) -> Result<Self::Transport, AppError> {
            Ok(SyncTransport {
                calls: Rc::clone(&self.calls),
                fail: self.fail,
            })
        }
        fn store(&mut self, _: &ReadRuntimeConfig) -> Result<Self::Repository, AppError> {
            Ok(self.store.take().unwrap())
        }
    }

    #[test]
    fn profile_sync_forces_fresh_sqlite_publication_and_preserves_failure() {
        use super::ProfileSyncOperations;
        use destiny_app::InventoryRepository;
        let calls = log();
        let mut foundations = SyncFoundations {
            store: Some(SqliteStore::open_in_memory().unwrap()),
            calls: Rc::clone(&calls),
            fail: false,
        };
        let mut runtime = compose(&mut foundations).unwrap();
        let first = runtime
            .service_mut()
            .run_profile_sync(
                FreshnessPolicy {
                    ttl_seconds: TTL_SECONDS,
                },
                NOW,
            )
            .unwrap();
        assert_eq!(first.previous_revision, None);
        assert_eq!(first.metadata.revision.value(), 1);
        let first_snapshot = runtime
            .service_mut()
            .repository()
            .load_current()
            .unwrap()
            .unwrap();
        calls.borrow_mut().clear();
        let later = Timestamp::new(NOW.seconds() + 1);
        let unchanged = runtime
            .service_mut()
            .run_profile_sync(
                FreshnessPolicy {
                    ttl_seconds: TTL_SECONDS,
                },
                later,
            )
            .unwrap();
        assert_eq!(unchanged.previous_revision, Some(first.metadata.revision));
        assert_eq!(unchanged.metadata.revision, first.metadata.revision);
        assert_eq!(unchanged.metadata.checked_at, NOW); // gateway acquisition clock, not caller's evaluation time
        assert!(calls.borrow().contains(&"membership"));
        assert!(calls.borrow().contains(&"profile"));
        let mut changed = destiny_app::InventorySnapshotInput {
            profile_currencies_coverage: first_snapshot.profile_currencies_coverage,
            stacks: first_snapshot.stacks,
            membership_id: first_snapshot.metadata.membership_id,
            membership_type: first_snapshot.metadata.membership_type,
            checked_at: NOW,
            source: first_snapshot.metadata.source,
            definition_version: first_snapshot.metadata.definition_version,
            characters: first_snapshot.characters,
            items: first_snapshot.items,
            plug_set_entries: first_snapshot.plug_set_entries,
        };
        changed.items.clear();
        runtime
            .service_mut()
            .repository_mut()
            .replace_current(changed)
            .unwrap();
        let different = runtime
            .service_mut()
            .run_profile_sync(
                FreshnessPolicy {
                    ttl_seconds: TTL_SECONDS,
                },
                later,
            )
            .unwrap();
        assert_eq!(different.previous_revision.unwrap().value(), 2);
        assert_eq!(different.metadata.revision.value(), 3);
        let committed = runtime
            .service_mut()
            .repository()
            .load_current()
            .unwrap()
            .unwrap();
        let store = runtime.service_mut().repository_mut();
        let replacement = SqliteStore::open_in_memory().unwrap();
        foundations.store = Some(std::mem::replace(store, replacement));
        foundations.fail = true;
        let mut failing = compose(&mut foundations).unwrap();
        let output = crate::runner::run_profile_sync(
            failing.service_mut(),
            OutputMode::Json,
            ReadFreshness {
                policy: FreshnessPolicy {
                    ttl_seconds: TTL_SECONDS,
                },
                now: later,
            },
        );
        assert_ne!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.contains("sentinel"));
        assert_eq!(
            failing
                .service_mut()
                .repository()
                .load_current()
                .unwrap()
                .unwrap(),
            committed
        );
    }

    struct PersonalTransport {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }
    impl ProfileTransport for PersonalTransport {
        fn get(
            &mut self,
            request: &ProfileHttpRequest<'_>,
        ) -> Result<ProfileHttpResponse, ProfileTransportError> {
            assert_eq!(
                request.url(),
                "https://www.bungie.net/Platform/Destiny2/3/Profile/999/Character/123/?components=202,204"
            );
            self.calls.borrow_mut().push("character-weekly");
            Ok(ProfileHttpResponse::new(200, None, Some("application/json".into()), Box::new(std::io::Cursor::new(br#"{"ErrorCode":1,"Response":{"progressions":{"data":{"milestones":{}}},"activities":{"data":{"availableActivities":[]}}}}"#.to_vec()))))
        }
    }
    struct PersonalFoundations {
        store: Option<SqliteStore>,
        calls: Rc<RefCell<Vec<&'static str>>>,
        missing_login: bool,
    }
    impl ReadFoundations for PersonalFoundations {
        type Source = destiny_app::StaticCredentialSource;
        type Transport = PersonalTransport;
        type Clock = ScriptedClock;
        type Repository = SqliteStore;
        fn config(&mut self) -> Result<ReadRuntimeConfig, AppError> {
            Ok(config())
        }
        fn credential_source(&mut self, _: &ReadRuntimeConfig) -> Result<Self::Source, AppError> {
            if self.missing_login {
                return Err(AppError::new(ErrorCategory::AuthorizationRequired));
            }
            Ok(destiny_app::StaticCredentialSource::new(credential()))
        }
        fn clock(&mut self) -> Result<Self::Clock, AppError> {
            Ok(ScriptedClock {
                now: Ok(NOW),
                log: Rc::clone(&self.calls),
            })
        }
        fn transport(&mut self) -> Result<Self::Transport, AppError> {
            Ok(PersonalTransport {
                calls: Rc::clone(&self.calls),
            })
        }
        fn store(&mut self, _: &ReadRuntimeConfig) -> Result<Self::Repository, AppError> {
            Ok(self.store.take().unwrap())
        }
    }

    #[test]
    fn personal_composition_gates_providers_and_reuses_authenticated_sqlite_projection() {
        use destiny_app::{InventoryRepository, ManifestGateway, WorldDefinitionRepository};
        for (populated, missing_login) in [(false, false), (true, false), (true, true)] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            if populated {
                let mut gateway = destiny_adapters::manifest::BungieManifestGateway::new(
                    destiny_adapters::manifest::ManifestApiKey::new(API_KEY_SENTINEL).unwrap(),
                    ManifestTransport,
                    ScriptedClock {
                        now: Ok(NOW),
                        log: log(),
                    },
                );
                let index = gateway.fetch_index().unwrap();
                let world = gateway.load_world_components(&index).unwrap();
                store
                    .publish_manifest(
                        destiny_app::DefinitionReplacementInput {
                            definitions: destiny_app::ManifestDefinitions {
                                version: destiny_app::DefinitionVersion::new("v1"),
                                definitions: vec![destiny_app::ItemDefinition {
                                    hash: destiny_app::ItemHash::new(1),
                                    bucket_hash: None,
                                    name: "Item".into(),
                                    item_type: None,
                                    tier: None,
                                    element: None,
                                    slot: None,
                                    class_type: None,
                                    plug_category: None,
                                    tags: Vec::new(),
                                    does_postmaster_pull_have_side_effects: None,
                                    is_instance_item: None,
                                    max_stack_size: None,
                                    stack_unique_label: None,
                                    version: destiny_app::DefinitionVersion::new("v1"),
                                }],
                            },
                            checked_at: NOW,
                            source: destiny_app::SourceMarker::Manifest,
                        },
                        world,
                    )
                    .unwrap();
                store
                    .replace_current(destiny_app::InventorySnapshotInput {
                        profile_currencies_coverage: Default::default(),
                        stacks: Vec::new(),
                        plug_set_entries: Vec::new(),
                        membership_id: destiny_app::MembershipId::new("999"),
                        membership_type: destiny_app::MembershipType::new(3),
                        checked_at: NOW,
                        source: destiny_app::SourceMarker::Profile,
                        definition_version: destiny_app::DefinitionVersion::new("v1"),
                        characters: vec![weekly_character("123")],
                        items: Vec::new(),
                    })
                    .unwrap();
            }
            let calls = log();
            let mut foundations = PersonalFoundations {
                store: Some(store),
                calls: Rc::clone(&calls),
                missing_login,
            };
            let result = super::compose_weekly(
                &mut foundations,
                WeeklyTransport {
                    calls: Rc::clone(&calls),
                },
                None,
            );
            if missing_login {
                assert_eq!(result.unwrap_err().code(), "authorization_required");
            } else if populated {
                let (character, weekly) = result.unwrap();
                assert_eq!(character.id.as_str(), "123");
                assert_eq!(weekly.definition_version.as_str(), "v1");
                assert_eq!(
                    &*calls.borrow(),
                    &["now", "milestones", "now", "character-weekly"]
                );
            } else {
                assert_eq!(result.unwrap_err().code(), "definitions_unavailable");
                assert!(!calls.borrow().contains(&"milestones"));
                assert!(!calls.borrow().contains(&"character-weekly"));
            }
        }
    }

    #[test]
    fn weekly_reset_composition_uses_published_sqlite_and_gates_empty_definitions() {
        use destiny_app::{ManifestGateway, WorldDefinitionRepository};
        for (populated, date, current_week) in [
            (false, None, true),
            (true, None, true),
            (true, Some("2026-10-06"), true),
            (true, Some("2026-09-29"), false),
        ] {
            // Private temporary SQLite database: no persistent state or credentials.
            let mut store = SqliteStore::open_in_memory().unwrap();
            if populated {
                let mut gateway = destiny_adapters::manifest::BungieManifestGateway::new(
                    destiny_adapters::manifest::ManifestApiKey::new(API_KEY_SENTINEL).unwrap(),
                    ManifestTransport,
                    ScriptedClock {
                        now: Ok(NOW),
                        log: log(),
                    },
                );
                let index = gateway.fetch_index().unwrap();
                let world = gateway.load_world_components(&index).unwrap();
                store
                    .publish_manifest(
                        destiny_app::DefinitionReplacementInput {
                            definitions: destiny_app::ManifestDefinitions {
                                version: destiny_app::DefinitionVersion::new("v1"),
                                definitions: vec![destiny_app::ItemDefinition {
                                    hash: destiny_app::ItemHash::new(1),
                                    bucket_hash: None,
                                    name: "Item".into(),
                                    item_type: None,
                                    tier: None,
                                    element: None,
                                    slot: None,
                                    class_type: None,
                                    plug_category: None,
                                    tags: Vec::new(),
                                    does_postmaster_pull_have_side_effects: None,
                                    is_instance_item: None,
                                    max_stack_size: None,
                                    stack_unique_label: None,
                                    version: destiny_app::DefinitionVersion::new("v1"),
                                }],
                            },
                            checked_at: NOW,
                            source: destiny_app::SourceMarker::Manifest,
                        },
                        world,
                    )
                    .unwrap();
            }
            let calls = log();
            let mut factory = super::ComposingManifestFactory::new(WeeklyFoundations {
                missing_login: false,
                inner: FakeManifestFoundations {
                    log: Rc::clone(&calls),
                    store: Some(store),
                },
            });
            let requested = date.map(|date| {
                destiny_adapters::calendar::instant_for_date(date, &chrono::Utc).unwrap()
            });
            let output =
                crate::runner::run_weekly_reset(&mut factory, OutputMode::Human, requested);
            if populated && !current_week {
                assert_eq!(output.exit_code, 0);
                assert!(
                    output
                        .stdout
                        .contains("Live data (observed) is only available for the current week.")
                );
                assert!(
                    output
                        .stdout
                        .contains("Featured raids (prediction): Deep Stone Crypt, Crota's End")
                );
                assert!(!calls.borrow().contains(&"milestones"));
            } else if populated {
                assert_eq!(output.exit_code, 0, "{}", output.stderr);
                assert!(
                    output.stdout.contains(
                        "Live milestones (observed)\nItem\n  - Item\n    modifiers: Item\n"
                    )
                );
                assert!(output.stdout.contains("Calendar (prediction)"));
                assert_eq!(
                    calls
                        .borrow()
                        .iter()
                        .filter(|step| **step == "milestones")
                        .count(),
                    1
                );
            } else {
                assert_eq!(output.exit_code, 33);
                assert_eq!(
                    output.stderr,
                    "error: definitions not loaded; run ghostlink sync-manifest\n"
                );
                assert!(!calls.borrow().contains(&"milestones"));
            }
        }
    }

    #[test]
    fn weekly_reset_missing_session_matches_authenticated_reads() {
        for mode in [OutputMode::Human, OutputMode::Json] {
            let calls = log();
            let mut weekly = super::ComposingManifestFactory::new(WeeklyFoundations {
                missing_login: true,
                inner: FakeManifestFoundations {
                    log: Rc::clone(&calls),
                    store: Some(SqliteStore::open_in_memory().unwrap()),
                },
            });
            let actual = crate::runner::run_weekly_reset(&mut weekly, mode, None);
            let mut read = PersonalFoundations {
                store: Some(SqliteStore::open_in_memory().unwrap()),
                calls: Rc::clone(&calls),
                missing_login: true,
            };
            let expected = crate::runner::ProcessOutput::failure(
                mode,
                compose(&mut read).err().unwrap().into(),
            );
            assert_eq!(actual, expected);
            assert_eq!(actual.exit_code, 10);
            assert!(!calls.borrow().contains(&"milestones"));
            assert!(!calls.borrow().contains(&"now"));
        }
    }

    #[test]
    fn manifest_foundations_render_both_outcomes_without_credentials() {
        for json in [false, true] {
            for unchanged in [false, true] {
                let store = SqliteStore::open_in_memory().unwrap();
                let log = log();
                let mut foundations = FakeManifestFoundations {
                    log: Rc::clone(&log),
                    store: Some(store),
                };
                if unchanged {
                    use destiny_app::{ManifestGateway, WorldDefinitionRepository};
                    let mut gateway = destiny_adapters::manifest::BungieManifestGateway::new(
                        destiny_adapters::manifest::ManifestApiKey::new(API_KEY_SENTINEL).unwrap(),
                        ManifestTransport,
                        ScriptedClock {
                            now: Ok(NOW),
                            log: Rc::clone(&log),
                        },
                    );
                    let index = gateway.fetch_index().unwrap();
                    let world = gateway.load_world_components(&index).unwrap();
                    foundations
                        .store
                        .as_mut()
                        .unwrap()
                        .publish_manifest(
                            destiny_app::DefinitionReplacementInput {
                                definitions: destiny_app::ManifestDefinitions {
                                    version: destiny_app::DefinitionVersion::new("v1"),
                                    definitions: vec![destiny_app::ItemDefinition {
                                        does_postmaster_pull_have_side_effects: None,
                                        is_instance_item: None,
                                        max_stack_size: None,
                                        stack_unique_label: None,
                                        hash: destiny_app::ItemHash::new(1),
                                        bucket_hash: None,
                                        name: "Item".into(),
                                        item_type: None,
                                        tier: None,
                                        element: None,
                                        slot: None,
                                        class_type: None,
                                        plug_category: None,
                                        tags: Vec::new(),
                                        version: destiny_app::DefinitionVersion::new("v1"),
                                    }],
                                },
                                checked_at: NOW,
                                source: destiny_app::SourceMarker::Manifest,
                            },
                            world,
                        )
                        .unwrap();
                    log.borrow_mut().clear();
                }
                let mode = if json {
                    OutputMode::Json
                } else {
                    OutputMode::Human
                };
                let mut factory = super::ComposingManifestFactory::new(foundations);
                let output = crate::runner::run_manifest(&mut factory, mode);
                assert_eq!(output.exit_code, 0, "{}", output.stderr);
                assert!(output.stderr.is_empty());
                let expected = match (json, unchanged) {
                    (false, false) => {
                        "manifest: updated\nversion: v1\ndefinitions: 1\nskipped: 1\nmilestones: 1\nmilestones_skipped: 1\nactivities: 1\nactivities_skipped: 1\nactivity_modifiers: 1\nactivity_modifiers_skipped: 1\nobjectives: 1\nobjectives_skipped: 1\nactivity_types: 1\nactivity_types_skipped: 1\nstat_definitions: 1\nstat_definitions_skipped: 1\n"
                    }
                    (false, true) => {
                        "manifest: unchanged\nversion: v1\ndefinitions: 1\nmilestones: 1\nactivities: 1\nactivity_modifiers: 1\nobjectives: 1\nactivity_types: 1\nstat_definitions: 1\n"
                    }
                    (true, false) => {
                        "{\"activities\":1,\"activities_skipped\":1,\"activity_modifiers\":1,\"activity_modifiers_skipped\":1,\"activity_types\":1,\"activity_types_skipped\":1,\"definitions\":1,\"kind\":\"sync_manifest\",\"milestones\":1,\"milestones_skipped\":1,\"objectives\":1,\"objectives_skipped\":1,\"skipped\":1,\"stat_definitions\":1,\"stat_definitions_skipped\":1,\"status\":\"updated\",\"version\":\"v1\"}\n"
                    }
                    (true, true) => {
                        "{\"activities\":1,\"activity_modifiers\":1,\"activity_types\":1,\"definitions\":1,\"kind\":\"sync_manifest\",\"milestones\":1,\"objectives\":1,\"stat_definitions\":1,\"status\":\"unchanged\",\"version\":\"v1\"}\n"
                    }
                };
                if json {
                    let mut actual: serde_json::Value =
                        serde_json::from_str(&output.stdout).unwrap();
                    assert_eq!(
                        actual["clarity"],
                        serde_json::json!({"status":"unavailable","version":null,"entries_stored":0,"entries_skipped":0})
                    );
                    actual.as_object_mut().unwrap().remove("clarity");
                    assert_eq!(
                        actual,
                        serde_json::from_str::<serde_json::Value>(expected).unwrap()
                    );
                } else {
                    assert_eq!(
                        output.stdout,
                        format!(
                            "{expected}clarity: unavailable\nclarity_version: unavailable\nclarity_entries_stored: 0\nclarity_entries_skipped: 0\n"
                        )
                    );
                }
                assert_eq!(
                    &log.borrow()[..],
                    ["config", "clock", "now", "transport", "store"]
                );
            }
        }
    }

    #[test]
    fn clarity_transport_failure_leaves_manifest_published() {
        use destiny_app::DefinitionRepository;
        let gateway = destiny_adapters::manifest::BungieManifestGateway::new(
            destiny_adapters::manifest::ManifestApiKey::new(API_KEY_SENTINEL).unwrap(),
            ManifestTransport,
            ScriptedClock {
                now: Ok(NOW),
                log: log(),
            },
        );
        let mut service =
            destiny_app::ManifestService::new(gateway, SqliteStore::open_in_memory().unwrap());
        let report = destiny_app::clarity::sync_manifest_and_clarity(
            &mut service,
            || {
                Ok(destiny_adapters::clarity::ClarityAdapter(
                    FailingClarityTransport,
                ))
            },
            NOW,
        )
        .unwrap();
        assert_eq!(report.clarity.status, "unavailable");
        assert_eq!(
            service
                .repository()
                .current_metadata()
                .unwrap()
                .unwrap()
                .definition_version
                .as_str(),
            "v1"
        );
    }

    #[test]
    fn broker_composition_selects_refreshing_credentials() {
        let mut values = HashMap::from([
            ("BUNGIE_API_KEY".into(), API_KEY_SENTINEL.into()),
            ("DESTINY_DATABASE_PATH".into(), DATABASE_SENTINEL.into()),
            ("DESTINY_DEFINITION_VERSION".into(), "manifest-v1".into()),
            ("DESTINY_FRESHNESS_TTL_SECONDS".into(), "900".into()),
            ("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into()),
        ]);
        values.insert(
            "GHOSTLINK_TOKEN_BROKER_URL".into(),
            "https://broker.test".into(),
        );
        let config = ReadRuntimeConfig::from_values(&values).unwrap();
        let source = super::compose_credentials(
            &config,
            FakeCredentialStore {
                result: Ok(Some(credential())),
            },
            super::configured_refresher,
            destiny_app::test_support::FakeClock::fixed(NOW),
        )
        .unwrap();
        assert!(matches!(
            source,
            destiny_app::ConfiguredCredentialSource::Refreshing(_)
        ));
    }

    #[test]
    fn confidential_composition_selects_refreshing_credentials_and_fails_early() {
        for missing in [false, true] {
            let log = log();
            let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
            foundations.config = Ok(ReadRuntimeConfig::from_values(&HashMap::from([
                ("BUNGIE_API_KEY".into(), API_KEY_SENTINEL.into()),
                ("DESTINY_DATABASE_PATH".into(), DATABASE_SENTINEL.into()),
                ("DESTINY_DEFINITION_VERSION".into(), "manifest-v1".into()),
                ("DESTINY_FRESHNESS_TTL_SECONDS".into(), "900".into()),
                ("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into()),
                ("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into()),
            ]))
            .unwrap());
            if missing {
                foundations.credential = Err(AppError::new(ErrorCategory::AuthorizationRequired));
            }
            let error = compose(&mut foundations).unwrap_err();
            assert_eq!(
                error.category(),
                if missing {
                    ErrorCategory::AuthorizationRequired
                } else {
                    ErrorCategory::LocalStorage
                }
            );
            let expected: &[&str] = if missing {
                &["config", "credential"]
            } else {
                &[
                    "config",
                    "credential",
                    "refreshing",
                    "clock",
                    "now",
                    "transport",
                    "store",
                ]
            };
            assert_eq!(&log.borrow()[..], expected);
        }
    }

    #[test]
    fn composition_follows_the_exact_fail_fast_order() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
        foundations.store = Some(Ok(SqliteStore::open_in_memory().expect("in-memory store")));

        let runtime = compose(&mut foundations).expect("composition succeeds");

        assert_eq!(
            &log.borrow()[..],
            ["config", "credential", "clock", "now", "transport", "store"]
        );
        assert_eq!(
            runtime.freshness(),
            ReadFreshness {
                policy: FreshnessPolicy {
                    ttl_seconds: TTL_SECONDS,
                },
                now: NOW,
            }
        );
    }

    #[test]
    fn a_composed_runtime_dispatches_a_valid_read_through_the_shared_runner() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
        foundations.store = Some(Ok(SqliteStore::open_in_memory().expect("in-memory store")));
        let mut runtime = compose(&mut foundations).expect("composition succeeds");

        // A missing projection forces a provider refresh; the scripted
        // transport is unreachable, so the read fails with the provider's
        // sanitized category rather than a composition fallback.
        let freshness = runtime.freshness();
        let output = dispatch(
            runtime.service_mut(),
            ReadRequest::ListCharacters,
            OutputMode::Human,
            freshness,
        );

        assert_eq!(output.exit_code, 21);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the provider is unavailable\n");
    }

    #[test]
    fn configuration_failure_stops_before_every_other_foundation() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
        foundations.config = Err(AppError::new(ErrorCategory::ConfigurationError));

        let error = compose(&mut foundations).expect_err("configuration is required");

        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        assert_eq!(&log.borrow()[..], ["config"]);
    }

    #[test]
    fn credential_failure_stops_before_clock_and_storage() {
        for (category, code, exit_code) in [
            (
                ErrorCategory::CredentialStoreUnavailable,
                "credential_store_unavailable",
                50,
            ),
            (
                ErrorCategory::AuthorizationRequired,
                "authorization_required",
                10,
            ),
        ] {
            let log = log();
            let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
            foundations.credential = Err(AppError::new(category));

            let error = compose(&mut foundations).expect_err("a credential is required");

            assert_eq!(error.category(), category);
            assert_eq!(&log.borrow()[..], ["config", "credential"]);
            assert_failure_is_sanitized(CliError::from(error), exit_code, code);
        }
    }

    #[test]
    fn clock_failure_stops_before_the_transport_and_store() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
        foundations.clock = Ok(ScriptedClock {
            now: Err(AppError::new(ErrorCategory::ClockOverflow)),
            log: Rc::clone(&log),
        });

        let error = compose(&mut foundations).expect_err("the clock must read");

        assert_eq!(error.category(), ErrorCategory::ClockOverflow);
        assert_eq!(&log.borrow()[..], ["config", "credential", "clock", "now"]);
        assert_failure_is_sanitized(CliError::from(error), 70, "internal_error");
    }

    #[test]
    fn transport_failure_stops_before_the_store() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));
        foundations.transport = Err(AppError::new(ErrorCategory::ConfigurationError));

        let error = compose(&mut foundations).expect_err("the transport must build");

        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        assert_eq!(
            &log.borrow()[..],
            ["config", "credential", "clock", "now", "transport"]
        );
    }

    #[test]
    fn storage_failure_happens_after_every_earlier_step() {
        let log = log();
        let mut foundations = FakeFoundations::store_failure(Rc::clone(&log));

        let error = compose(&mut foundations).expect_err("the store is scripted to fail");

        assert_eq!(error.category(), ErrorCategory::LocalStorage);
        assert_eq!(
            &log.borrow()[..],
            ["config", "credential", "clock", "now", "transport", "store"]
        );
    }

    #[test]
    fn every_composition_failure_maps_to_its_typed_cli_outcome() {
        // Each case scripts exactly one foundation failure and proves the
        // production factory surfaces the stable, sanitized CLI outcome.
        let cases: [FailureCase; 5] = [
            (
                "configuration_error",
                |foundations| {
                    foundations.config = Err(AppError::new(ErrorCategory::ConfigurationError));
                },
                "configuration_error",
                52,
            ),
            (
                "credential_store_unavailable",
                |foundations| {
                    foundations.credential =
                        Err(AppError::new(ErrorCategory::CredentialStoreUnavailable));
                },
                "credential_store_unavailable",
                50,
            ),
            (
                "authorization_required",
                |foundations| {
                    foundations.credential =
                        Err(AppError::new(ErrorCategory::AuthorizationRequired));
                },
                "authorization_required",
                10,
            ),
            (
                "internal_error",
                |foundations| {
                    foundations.clock = Ok(ScriptedClock {
                        now: Err(AppError::new(ErrorCategory::ClockOverflow)),
                        log: Rc::new(RefCell::new(Vec::new())),
                    });
                },
                "internal_error",
                70,
            ),
            ("local_storage", |_| {}, "local_storage", 51),
        ];

        for (name, apply, code, exit_code) in cases {
            let mut foundations = FakeFoundations::store_failure(log());
            apply(&mut foundations);
            let error = ComposingRuntimeFactory::new(foundations)
                .build()
                .expect_err("the factory must report a sanitized failure");
            assert_failure_is_sanitized(error.clone(), exit_code, code);
            assert_eq!(error.code(), code, "case: {name}");
        }
    }

    #[test]
    fn a_missing_stored_credential_requires_authorization() {
        let store = FakeCredentialStore { result: Ok(None) };
        let error = load_required_credential(&store).expect_err("missing credential fails");
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        assert_failure_is_sanitized(CliError::from(error), 10, "authorization_required");
    }

    #[test]
    fn a_present_stored_credential_is_returned_unchanged() {
        let expected = credential();
        let store = FakeCredentialStore {
            result: Ok(Some(expected.clone())),
        };
        assert_eq!(
            load_required_credential(&store).expect("present credential"),
            expected
        );
    }

    #[test]
    fn a_credential_store_failure_propagates_its_own_category() {
        let store = FakeCredentialStore {
            result: Err(AppError::new(ErrorCategory::CredentialStoreUnavailable)),
        };
        assert_eq!(
            load_required_credential(&store)
                .expect_err("store failure")
                .category(),
            ErrorCategory::CredentialStoreUnavailable
        );
    }

    #[test]
    fn the_production_factory_composes_the_real_adapters() {
        // Type-level proof that the production factory yields a runtime the
        // runner can dispatch, without reading the environment or a keyring.
        fn assert_runtime_factory<F: RuntimeFactory>() {}
        assert_runtime_factory::<ComposingRuntimeFactory<ProductionFoundations>>();
    }
}
