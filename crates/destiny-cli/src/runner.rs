//! The thin command runner and the sanitized process boundary.
//!
//! The runner validates one parsed command line, dispatches exactly one read
//! through the shared [`QueryOperations`] surface, and renders the shared
//! result in the selected mode. It never reproduces freshness, authorization,
//! or synchronization logic: it forwards the caller-supplied
//! [`ReadFreshness`] to the service, which owns the skip/refresh decision, and
//! it renders whatever metadata that service returns.
//!
//! The process boundary captures clap syntax and type failures and converts
//! them into the same sanitized `invalid_request`/2 contract as a rejected
//! request, in both human and JSON modes. clap's default error prose is never
//! emitted, and no rejected argument, option, or value is ever included in the
//! rendered failure.

use std::ffi::OsString;

use clap::Parser;

use destiny_app::{FreshnessPolicy, QueryOperations, Timestamp};

use crate::command::{Cli, CliError, ReadRequest, SUCCESS_EXIT_CODE, inspection_outcome};
use crate::output::{
    OutputMode, render_characters, render_error, render_inspection, render_search,
};
use crate::runtime::RuntimeFactory;

/// The caller-supplied freshness inputs for one read.
///
/// The CLI forwards these verbatim to the shared service. It never selects a
/// sync reason, never reads a clock, and never decides whether a projection is
/// fresh or stale: that decision belongs to `ProfileSyncService`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadFreshness {
    /// The service-owned freshness policy.
    pub policy: FreshnessPolicy,
    /// The explicit current time the service evaluates freshness against.
    pub now: Timestamp,
}

/// One fully rendered process result.
///
/// `stdout` and `stderr` hold the exact bytes to write, already selected for
/// the requested output mode and already sanitized. `exit_code` is the stable
/// typed status from the shared outcome matrix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutput {
    /// The success text for stdout; empty for a failure.
    pub stdout: String,
    /// The sanitized failure text for stderr; empty for a success.
    pub stderr: String,
    /// The stable process exit status.
    pub exit_code: i32,
}

impl ProcessOutput {
    /// Builds a successful result whose body is written to stdout.
    fn success(body: String) -> Self {
        Self {
            stdout: format!("{body}\n"),
            stderr: String::new(),
            exit_code: SUCCESS_EXIT_CODE,
        }
    }

    /// Builds a sanitized failure written to stderr.
    fn failure(mode: OutputMode, error: CliError) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("{}\n", render_error(mode, &error)),
            exit_code: error.exit_code(),
        }
    }

    /// Writes both streams and returns the stable exit status.
    pub fn emit(&self) -> std::process::ExitCode {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(self.stdout.as_bytes());
        let _ = stdout.flush();
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(self.stderr.as_bytes());
        let _ = stderr.flush();
        std::process::ExitCode::from(u8::try_from(self.exit_code).unwrap_or(1))
    }
}

/// Parses the process arguments through the sanitized boundary.
///
/// clap's default error prose never reaches the process. A syntax or type
/// failure becomes the stable `invalid_request`/2 result in the mode selected
/// by an explicitly present global `--json` flag. That flag is detected from
/// the raw argument tokens only, and no rejected value is ever rendered. Help
/// and version output are passed through on stdout with a success status.
pub fn parse_process<I, T>(args: I) -> Result<Cli, ProcessOutput>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    match Cli::try_parse_from(args.iter().cloned()) {
        Ok(cli) => Ok(cli),
        Err(error) => Err(parse_failure(json_mode(&args), &error)),
    }
}

/// Validates one parsed command line and dispatches it exactly once.
///
/// Validation happens before any service call, so an invalid request can never
/// reach the shared service.
pub fn run_cli<S>(cli: Cli, service: &mut S, freshness: ReadFreshness) -> ProcessOutput
where
    S: QueryOperations + ?Sized,
{
    let mode = cli.output_mode();
    match cli.into_read_request() {
        Ok(request) => dispatch(service, request, mode, freshness),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Parses the process arguments and runs the requested read exactly once.
pub fn run<S, I, T>(service: &mut S, args: I, freshness: ReadFreshness) -> ProcessOutput
where
    S: QueryOperations + ?Sized,
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    match parse_process(args) {
        Ok(cli) => run_cli(cli, service, freshness),
        Err(output) => output,
    }
}

/// Dispatches one already-validated read through the shared trait exactly once.
///
/// The shared result, including its metadata, is rendered in the selected mode.
/// A missing inspection is mapped through [`inspection_outcome`] to the stable
/// `item_not_found` outcome rather than a successful empty object.
pub fn dispatch<S>(
    service: &mut S,
    request: ReadRequest,
    mode: OutputMode,
    freshness: ReadFreshness,
) -> ProcessOutput
where
    S: QueryOperations + ?Sized,
{
    let rendered: Result<String, CliError> = match request {
        ReadRequest::ListCharacters => service
            .list_characters(freshness.policy, freshness.now)
            .map(|result| render_characters(mode, &result))
            .map_err(CliError::from),
        ReadRequest::SearchInventory(request) => service
            .search_inventory(&request, freshness.policy, freshness.now)
            .map(|result| render_search(mode, &result))
            .map_err(CliError::from),
        ReadRequest::InspectItem(request) => service
            .inspect_item(&request, freshness.policy, freshness.now)
            .map_err(CliError::from)
            .and_then(inspection_outcome)
            .map(|inspection| render_inspection(mode, &inspection)),
    };

    match rendered {
        Ok(body) => ProcessOutput::success(body),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// The production process boundary of the binary.
///
/// It builds the production runtime factory, which performs no acquisition on
/// construction, and delegates to [`run_binary_with`]. The request is parsed and
/// semantically validated before the factory is asked to build anything, so an
/// invalid request never reads configuration, a credential store, the database,
/// a clock, or a transport. A valid request is dispatched through the existing
/// runner path over the composed [`destiny_app::QueryService`].
pub fn run_binary<I, T>(args: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut factory = crate::runtime::production_runtime_factory();
    run_binary_with(&mut factory, args)
}

/// Runs one command line against an injectable runtime factory.
///
/// The order is fixed: parse, then semantically validate the read request, and
/// only then ask the factory for the runtime. A parse or validation failure is
/// reported before the factory is called at all, so an invalid request can
/// never trigger a configuration read, a credential lookup, a keyring access, a
/// database open, a clock read, or a network call. A runtime failure is mapped
/// through the same stable, sanitized outcome matrix as any other failure.
pub fn run_binary_with<F, I, T>(factory: &mut F, args: I) -> ProcessOutput
where
    F: RuntimeFactory,
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let cli = match parse_process(args) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    let mode = cli.output_mode();
    let request = match cli.into_read_request() {
        Ok(request) => request,
        Err(error) => return ProcessOutput::failure(mode, error),
    };
    match factory.build() {
        Ok(mut runtime) => {
            let freshness = runtime.freshness();
            dispatch(runtime.service_mut(), request, mode, freshness)
        }
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Selects the output mode from an explicit `--json` token in the raw argv.
///
/// The check compares one token to the literal flag and never renders the
/// argument, so a rejected value can never leak through error detection.
fn json_mode(args: &[OsString]) -> OutputMode {
    if args
        .iter()
        .take_while(|argument| argument.to_str() != Some("--"))
        .any(|argument| argument.to_str() == Some("--json"))
    {
        OutputMode::Json
    } else {
        OutputMode::Human
    }
}

/// Converts one clap failure into the sanitized process result.
fn parse_failure(mode: OutputMode, error: &clap::Error) -> ProcessOutput {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp
        | clap::error::ErrorKind::DisplayVersion
        | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => ProcessOutput {
            stdout: error.render().to_string(),
            stderr: String::new(),
            exit_code: SUCCESS_EXIT_CODE,
        },
        _ => ProcessOutput::failure(mode, CliError::invalid_request()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProcessOutput, ReadFreshness, dispatch, parse_process, run, run_binary, run_binary_with,
    };
    use crate::command::{Cli, CliError, ReadRequest};
    use crate::output::OutputMode;
    use crate::runtime::{ReadRuntime, RuntimeFactory};
    use destiny_app::{
        AppError, Character, CharacterId, CharacterListResult, Dataset, DefinitionVersion,
        ErrorCategory, FreshnessPolicy, FreshnessStatus, InspectItemRequest, InventorySearchHit,
        InventorySearchResult, ItemDefinition, ItemHash, ItemInspection, ItemInstance,
        ItemInstanceId, ItemLocation, QueryMetadata, QueryOperations, Resolution,
        SearchInventoryRequest, SourceMarker, SyncMetadata, SyncRevision, Timestamp,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(10_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

    /// The exact read a recording service observed.
    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Recorded {
        List,
        Search(SearchInventoryRequest),
        Inspect(InspectItemRequest),
    }

    /// The call log of a recording service.
    ///
    /// It is shared through an `Rc<RefCell<_>>` so a runtime factory can hand
    /// out a clone of the service and the test can still observe every call the
    /// dispatched clone made.
    type RecordedCalls = Rc<RefCell<Vec<(Recorded, FreshnessPolicy, Timestamp)>>>;

    /// A recording fake that returns canned shared results and never touches I/O.
    #[derive(Clone, Debug)]
    struct RecordingService {
        calls: RecordedCalls,
        list: Result<CharacterListResult, AppError>,
        search: Result<InventorySearchResult, AppError>,
        inspect: Result<Resolution<ItemInspection>, AppError>,
    }

    impl RecordingService {
        fn new() -> Self {
            Self {
                calls: Rc::new(RefCell::new(Vec::new())),
                list: Ok(character_list()),
                search: Ok(search_result()),
                inspect: Ok(Resolution::Unique(inspection())),
            }
        }

        fn recorded(&self) -> Vec<(Recorded, FreshnessPolicy, Timestamp)> {
            self.calls.borrow().clone()
        }

        fn record(&self, read: Recorded, policy: FreshnessPolicy, now: Timestamp) {
            self.calls.borrow_mut().push((read, policy, now));
        }
    }

    impl QueryOperations for RecordingService {
        fn list_characters(
            &mut self,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<CharacterListResult, AppError> {
            self.record(Recorded::List, policy, now);
            self.list.clone()
        }

        fn search_inventory(
            &mut self,
            request: &SearchInventoryRequest,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<InventorySearchResult, AppError> {
            self.record(Recorded::Search(request.clone()), policy, now);
            self.search.clone()
        }

        fn inspect_item(
            &mut self,
            request: &InspectItemRequest,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<Resolution<ItemInspection>, AppError> {
            self.record(Recorded::Inspect(request.clone()), policy, now);
            self.inspect.clone()
        }
    }

    fn freshness() -> ReadFreshness {
        ReadFreshness {
            policy: TTL,
            now: NOW,
        }
    }

    /// A runtime factory that records its build count and injects a service.
    #[derive(Clone)]
    struct FakeFactory {
        builds: Rc<Cell<usize>>,
        service: RecordingService,
        freshness: ReadFreshness,
        failure: Option<CliError>,
    }

    impl FakeFactory {
        fn new(service: RecordingService) -> Self {
            Self {
                builds: Rc::new(Cell::new(0)),
                service,
                freshness: freshness(),
                failure: None,
            }
        }

        fn build_count(&self) -> usize {
            self.builds.get()
        }
    }

    impl RuntimeFactory for FakeFactory {
        type Service = RecordingService;

        fn build(&mut self) -> Result<ReadRuntime<Self::Service>, CliError> {
            self.builds.set(self.builds.get() + 1);
            match self.failure {
                Some(error) => Err(error),
                None => Ok(ReadRuntime::new(self.service.clone(), self.freshness)),
            }
        }
    }

    fn metadata(freshness: FreshnessStatus) -> QueryMetadata {
        QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(9_999),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(3),
                definition_version: DefinitionVersion::new("manifest-v1"),
            },
            freshness,
        }
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.to_string(),
            item_type: Some("Rocket Launcher".to_string()),
            tier: None,
            element: None,
            tags: Vec::new(),
            version: DefinitionVersion::new("manifest-v1"),
        }
    }

    fn instance(identifier: &str, hash: i64) -> ItemInstance {
        ItemInstance {
            instance_id: ItemInstanceId::new(identifier),
            item_hash: ItemHash::new(hash),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: Some(1_350),
            perks: Vec::new(),
        }
    }

    fn character_list() -> CharacterListResult {
        CharacterListResult {
            characters: vec![Character {
                id: CharacterId::new("warlock"),
                name: Some("Nova".to_string()),
                class: Some("Warlock".to_string()),
                race: None,
                gender: None,
                light: Some(1_810),
            }],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn search_result() -> InventorySearchResult {
        InventorySearchResult {
            candidates: vec![definition(1, "Gjallarhorn")],
            hits: vec![InventorySearchHit {
                definition: definition(1, "Gjallarhorn"),
                instance: instance("inst-1", 1),
            }],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn inspection() -> ItemInspection {
        ItemInspection {
            definition: definition(1, "Gjallarhorn"),
            instance: instance("inst-1", 1),
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn argv(arguments: &[&str]) -> Vec<String> {
        let mut argv = vec!["destiny-cli".to_string()];
        argv.extend(arguments.iter().map(|argument| (*argument).to_string()));
        argv
    }

    #[test]
    fn clap_syntax_errors_are_sanitized_and_never_call_the_service() {
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["search-inventory", "--limit", "not-a-number"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn clap_errors_honor_global_json_without_echoing_arguments() {
        let sentinel = "--secret-sentinel-value";
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["--json", "inspect-item", "--unknown", sentinel]),
            freshness(),
        );

        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"invalid_request\",\"message\":\"the request arguments are invalid\"}}\n"
        );
        assert!(!output.stderr.contains(sentinel));
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn json_after_the_option_terminator_does_not_select_json_error_mode() {
        // `--json` after `--` is a positional value, not the global flag, so a
        // rejected command line must still render the human error line. Detecting
        // the flag past the terminator would silently switch the failure mode.
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["list-characters", "--", "--json"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn json_detection_also_covers_a_subcommand_local_flag() {
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["list-characters", "--json", "--unknown"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 2);
        assert!(output.stderr.starts_with("{\"error\":"));
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn request_validation_failures_never_call_the_service() {
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["search-inventory", "--limit", "0"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn help_is_passed_through_without_clap_error_prose() {
        let output = run(&mut RecordingService::new(), argv(&["--help"]), freshness());

        assert_eq!(output.exit_code, 0);
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn list_characters_dispatches_once_with_the_caller_freshness() {
        let mut service = RecordingService::new();
        let output = run(&mut service, argv(&["list-characters"]), freshness());

        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        assert!(output.stdout.starts_with("characters: 1\n"));
        assert!(output.stdout.contains("id: warlock"));
        assert!(
            output
                .stdout
                .ends_with("revision: 3\nchecked_at: 9999\nfreshness: fresh\n")
        );
        assert_eq!(service.recorded(), [(Recorded::List, TTL, NOW)]);
    }

    #[test]
    fn search_dispatches_the_exact_validated_request_once() {
        let expected = SearchInventoryRequest {
            definitions: destiny_app::DefinitionSearchQuery {
                text: Some(destiny_app::SearchTerm::new("gjallar").unwrap()),
                item_type: None,
                tags: Vec::new(),
                limit: 5,
            },
            instances: Some(destiny_app::InstanceSearchQuery {
                item_hashes: destiny_app::ItemHashFilter::AnyOf(vec![ItemHash::new(1)]),
                perk_hash: None,
                character_id: None,
                limit: 5,
            }),
        };
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&[
                "search-inventory",
                "--text",
                "gjallar",
                "--item-hash",
                "1",
                "--limit",
                "5",
            ]),
            freshness(),
        );

        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("candidates: 1"));
        assert!(output.stdout.ends_with("freshness: fresh\n"));
        assert_eq!(service.recorded(), [(Recorded::Search(expected), TTL, NOW)]);
    }

    #[test]
    fn inspect_dispatches_once_and_renders_the_shared_metadata() {
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["--json", "inspect-item", "inst-1"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.trim()).expect("one valid JSON object");
        assert_eq!(parsed["kind"], "item_inspection");
        assert_eq!(parsed["metadata"]["revision"], 3);
        assert_eq!(parsed["metadata"]["checked_at"], 9_999);
        assert_eq!(parsed["metadata"]["freshness"], "fresh");
        assert_eq!(
            service.recorded(),
            [(
                Recorded::Inspect(InspectItemRequest {
                    instance_id: ItemInstanceId::new("inst-1"),
                }),
                TTL,
                NOW,
            )]
        );
    }

    #[test]
    fn a_missing_inspection_is_a_typed_not_found_without_stdout() {
        let mut service = RecordingService::new();
        service.inspect = Ok(Resolution::Missing);
        let output = run(
            &mut service,
            argv(&["inspect-item", "inst-absent"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 32);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "error: the requested item instance was not found\n"
        );
        assert_eq!(service.recorded().len(), 1);
    }

    #[test]
    fn application_errors_map_to_their_stable_outcome() {
        let mut service = RecordingService::new();
        service.search = Err(AppError::new(ErrorCategory::ProviderRateLimit));
        let output = run(
            &mut service,
            argv(&["--json", "search-inventory"]),
            freshness(),
        );

        assert_eq!(output.exit_code, 20);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"provider_rate_limit\",\"message\":\"the provider rate limit was reached\"}}\n"
        );
        assert_eq!(service.recorded().len(), 1);
    }

    #[test]
    fn dispatch_accepts_a_trait_object_and_calls_it_once() {
        let mut service = RecordingService::new();
        let target: &mut dyn QueryOperations = &mut service;
        let output = dispatch(
            target,
            ReadRequest::ListCharacters,
            OutputMode::Human,
            freshness(),
        );

        assert_eq!(output.exit_code, 0);
        assert_eq!(service.recorded(), [(Recorded::List, TTL, NOW)]);
    }

    #[test]
    fn invalid_arguments_never_build_the_runtime() {
        let service = RecordingService::new();
        let mut factory = FakeFactory::new(service.clone());

        // A clap syntax failure.
        let output = run_binary_with(&mut factory, argv(&["inspect-item", "--unknown"]));
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");

        // A parsed but semantically invalid request.
        let output = run_binary_with(&mut factory, argv(&["search-inventory", "--limit", "0"]));
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");

        // A missing required argument is rejected before any runtime work too.
        let output = run_binary_with(&mut factory, argv(&["inspect-item"]));
        assert_eq!(output.exit_code, 2);

        assert_eq!(factory.build_count(), 0);
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn a_valid_request_dispatches_through_the_built_runtime_with_its_policy_and_time() {
        let policy = FreshnessPolicy { ttl_seconds: 60 };
        let now = Timestamp::new(123_456);
        let service = RecordingService::new();
        let mut factory = FakeFactory::new(service.clone());
        factory.freshness = ReadFreshness { policy, now };

        let output = run_binary_with(&mut factory, argv(&["list-characters"]));

        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        assert_eq!(factory.build_count(), 1);
        assert_eq!(service.recorded(), [(Recorded::List, policy, now)]);
    }

    #[test]
    fn a_runtime_failure_is_sanitized_in_both_output_modes() {
        // A runtime that cannot be built dispatches nothing and reports the
        // stable typed outcome on the selected stream only.
        let service = RecordingService::new();
        let mut factory = FakeFactory::new(service.clone());
        factory.failure = Some(CliError::from_category(
            ErrorCategory::CredentialStoreUnavailable,
        ));

        let output = run_binary_with(&mut factory, argv(&["list-characters"]));
        assert_eq!(output.exit_code, 50);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: credential storage is unavailable\n");
        assert_eq!(factory.build_count(), 1);
        assert!(service.recorded().is_empty());

        let mut factory = FakeFactory::new(service.clone());
        factory.failure = Some(CliError::from_category(ErrorCategory::ConfigurationError));
        let output = run_binary_with(&mut factory, argv(&["--json", "list-characters"]));
        assert_eq!(output.exit_code, 52);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"configuration_error\",\"message\":\"required runtime configuration is missing or invalid\"}}\n"
        );
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn the_production_boundary_rejects_invalid_arguments_without_reading_the_environment() {
        // `run_binary` builds only the infallible production factory. Because a
        // rejected request never reaches the factory, this touches no
        // environment variable, keyring, database, clock, or transport.
        let output: ProcessOutput = run_binary(argv(&["inspect-item", "--unknown"]));
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
    }

    #[test]
    fn parse_process_returns_the_parsed_command_for_valid_arguments() {
        let cli: Cli = parse_process(argv(&["list-characters"])).ok().unwrap();
        assert_eq!(cli.output_mode(), OutputMode::Human);

        let invalid = parse_process(argv(&["list-characters", "--unknown"])).unwrap_err();
        assert_eq!(invalid.exit_code, CliError::invalid_request().exit_code());
    }
}
