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
/// A `QueryService` cannot yet be composed from the public `destiny-app`
/// foundations: `load_config` is crate-visible and no public function builds a
/// service over the production adapters, which are not part of this crate's
/// dependency surface either. Rather than invent an adapter or business logic
/// here, the binary validates the process arguments, retains the sanitized
/// startup diagnostic, and reports the stable `unsupported_operation` outcome
/// for a read it cannot serve. The `tests::binary_boundary_documents_the_composition_limitation`
/// test records this boundary.
pub fn run_binary<I, T>(args: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let cli = match parse_process(args) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    let mode = cli.output_mode();
    // Validate the read before the startup check so an invalid request keeps the
    // same `invalid_request`/2 contract the tested runner produces, whether or
    // not the runtime could serve it.
    if let Err(error) = cli.into_read_request() {
        return ProcessOutput::failure(mode, error);
    }
    binary_outcome(mode, destiny_app::startup())
}

/// Maps a startup result into the stable process outcome.
///
/// On success the read runtime cannot be composed, so the outcome is the
/// sanitized `unsupported_operation`; on failure the sanitized configuration
/// diagnostic from [`destiny_app::startup`] is preserved unchanged.
fn binary_outcome(mode: OutputMode, startup: Result<(), destiny_app::AppError>) -> ProcessOutput {
    match startup {
        Ok(()) => ProcessOutput::failure(
            mode,
            CliError::from_category(destiny_app::ErrorCategory::UnsupportedOperation),
        ),
        Err(error) => ProcessOutput::failure(mode, CliError::from_app(error)),
    }
}

/// Selects the output mode from an explicit `--json` token in the raw argv.
///
/// The check compares one token to the literal flag and never renders the
/// argument, so a rejected value can never leak through error detection.
fn json_mode(args: &[OsString]) -> OutputMode {
    if args
        .iter()
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
    use super::{ProcessOutput, ReadFreshness, dispatch, parse_process, run, run_binary};
    use crate::command::{Cli, CliError, ReadRequest};
    use crate::output::OutputMode;
    use destiny_app::{
        AppError, Character, CharacterId, CharacterListResult, Dataset, DefinitionVersion,
        ErrorCategory, FreshnessPolicy, FreshnessStatus, InspectItemRequest, InventorySearchHit,
        InventorySearchResult, ItemDefinition, ItemHash, ItemInspection, ItemInstance,
        ItemInstanceId, ItemLocation, QueryMetadata, QueryOperations, Resolution,
        SearchInventoryRequest, SourceMarker, SyncMetadata, SyncRevision, Timestamp,
    };

    const NOW: Timestamp = Timestamp::new(10_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

    /// The exact read a recording service observed.
    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Recorded {
        List,
        Search(SearchInventoryRequest),
        Inspect(InspectItemRequest),
    }

    /// A recording fake that returns canned shared results and never touches I/O.
    #[derive(Clone, Debug)]
    struct RecordingService {
        calls: Vec<(Recorded, FreshnessPolicy, Timestamp)>,
        list: Result<CharacterListResult, AppError>,
        search: Result<InventorySearchResult, AppError>,
        inspect: Result<Resolution<ItemInspection>, AppError>,
    }

    impl RecordingService {
        fn new() -> Self {
            Self {
                calls: Vec::new(),
                list: Ok(character_list()),
                search: Ok(search_result()),
                inspect: Ok(Resolution::Unique(inspection())),
            }
        }

        fn recorded(&self) -> &[(Recorded, FreshnessPolicy, Timestamp)] {
            &self.calls
        }
    }

    impl QueryOperations for RecordingService {
        fn list_characters(
            &mut self,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<CharacterListResult, AppError> {
            self.calls.push((Recorded::List, policy, now));
            self.list.clone()
        }

        fn search_inventory(
            &mut self,
            request: &SearchInventoryRequest,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<InventorySearchResult, AppError> {
            self.calls
                .push((Recorded::Search(request.clone()), policy, now));
            self.search.clone()
        }

        fn inspect_item(
            &mut self,
            request: &InspectItemRequest,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<Resolution<ItemInspection>, AppError> {
            self.calls
                .push((Recorded::Inspect(request.clone()), policy, now));
            self.inspect.clone()
        }
    }

    fn freshness() -> ReadFreshness {
        ReadFreshness {
            policy: TTL,
            now: NOW,
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
    fn binary_boundary_documents_the_composition_limitation() {
        // A successful startup still cannot compose a read service from public
        // foundations, so the boundary reports the sanitized unsupported
        // outcome instead of inventing an adapter or business logic.
        let output = super::binary_outcome(OutputMode::Human, Ok(()));
        assert_eq!(output.exit_code, 40);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the operation is unsupported\n");

        // A startup failure keeps the sanitized configuration diagnostic.
        let output = super::binary_outcome(
            OutputMode::Human,
            Err(AppError::new(ErrorCategory::ConfigurationError)),
        );
        assert_eq!(output.exit_code, 52);
        assert_eq!(
            output.stderr,
            "error: required runtime configuration is missing or invalid\n"
        );
    }

    #[test]
    fn parse_process_returns_the_parsed_command_for_valid_arguments() {
        let cli: Cli = parse_process(argv(&["list-characters"])).ok().unwrap();
        assert_eq!(cli.output_mode(), OutputMode::Human);

        let invalid = parse_process(argv(&["list-characters", "--unknown"])).unwrap_err();
        assert_eq!(invalid.exit_code, CliError::invalid_request().exit_code());
    }

    #[test]
    fn run_binary_rejects_invalid_arguments_before_startup() {
        let output: ProcessOutput = run_binary(argv(&["inspect-item"]));
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");

        // A parsed but invalid request is rejected before the startup check, so
        // it keeps the same contract without depending on the environment.
        let output: ProcessOutput =
            run_binary(vec!["destiny-cli", "search-inventory", "--limit", "0"]);
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
    }
}
