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
//! 6. [`BungieProfileGateway`] is built with the configured definition version;
//! 7. the durable [`SqliteStore`] is opened at the configured path;
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
    type Repository: InventoryRepository + SearchRepository + DefinitionRepository;

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
        compose_credentials(
            config,
            store,
            |client| {
                destiny_adapters::BungieTokenExchange::confidential(
                    client.client_id(),
                    "ghostlink://oauth/callback",
                    client.client_secret().clone(),
                    destiny_adapters::HttpTokenTransport::new()?,
                    SystemClock::new(),
                )
            },
            SystemClock::new(),
        )
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

/// The injectable seam the process boundary uses to obtain a read runtime.
///
/// Production supplies the real composition; tests supply a scripted service,
/// policy, and time. A build failure is a stable, sanitized [`CliError`], and a
/// failure is the only way [`RuntimeFactory::build`] can avoid returning a
/// dispatchable runtime.
pub trait RuntimeFactory {
    /// The read service the runner dispatches through.
    type Service: QueryOperations;

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
    let source = foundations.credential_source(&config)?;
    let clock = foundations.clock()?;
    // Exactly one reading supplies the freshness evaluation time; the shared
    // services own the skip/refresh decision against it.
    let now = clock.now()?;
    let transport = foundations.transport()?;
    let api_key = ProfileApiKey::new(config.api_key().expose_secret())?;
    let gateway = BungieProfileGateway::new(
        transport,
        clock,
        source,
        api_key,
        config.definition_version().clone(),
    )?;
    let repository = foundations.store(&config)?;
    let service = QueryService::new(ProfileSyncService::new(gateway, repository));
    let freshness = ReadFreshness {
        policy: config.freshness_policy(),
        now,
    };
    Ok(ReadRuntime::new(service, freshness))
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
            self.now
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
            self.transport
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
            assert_failure_is_sanitized(error, exit_code, code);
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
