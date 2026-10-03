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

use destiny_app::{
    AuthorizationOutcome, ErrorCategory, FreshnessPolicy, QueryOperations, Timestamp,
};

use crate::command::{Cli, CliError, Command, ReadRequest, SUCCESS_EXIT_CODE, inspection_outcome};
use crate::login::{LoginFactory, LoginOperations};
use crate::output::{
    OutputMode, render_characters, render_error, render_inspection, render_login_authorized,
    render_login_instruction, render_search,
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
/// It builds the production read and login factories, neither of which acquires
/// anything on construction, and delegates to [`run_binary_with`]. The request is
/// parsed and then routed before either factory is asked to build anything, so an
/// invalid command line never reads configuration, binds a callback port, touches
/// a credential store, opens a database, reads a clock, or reaches a transport.
/// A valid read is dispatched through the existing runner path over the composed
/// [`destiny_app::QueryService`]; a valid login is dispatched through the login
/// composition.
pub fn run_binary<I, T>(args: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut read_factory = crate::runtime::production_runtime_factory();
    let mut login_factory = crate::login::ProductionLoginFactory::new();
    run_binary_with(
        &mut read_factory,
        &mut login_factory,
        &mut present_to_stdout,
        args,
    )
}

/// Runs one command line against injectable read and login factories.
///
/// The order is fixed: parse, then route on the parsed command. A read command is
/// semantically validated before the read factory is asked for a runtime, and a
/// login command is never validated as a read; a login command is passed to the
/// login factory before the read factory is touched. A parse, validation, or
/// factory failure is reported before anything else is built, so a rejected
/// request can never trigger a configuration read, a loopback bind, a keyring
/// access, a database open, a clock read, or a network call.
///
/// The `present` sink receives any operator instruction the selected login mode
/// produced; it exists so the instruction reaches the human before the callback is
/// awaited, and so tests can observe the exact text without a terminal.
pub fn run_binary_with<F, G, P, I, T>(
    read_factory: &mut F,
    login_factory: &mut G,
    present: &mut P,
    args: I,
) -> ProcessOutput
where
    F: RuntimeFactory,
    G: LoginFactory,
    P: FnMut(&str) -> Result<(), CliError>,
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut manifest =
        crate::runtime::ComposingManifestFactory::new(crate::runtime::ProductionFoundations);
    run_binary_with_manifest(read_factory, login_factory, &mut manifest, present, args)
}

/// Injectable routing including the public Manifest path.
pub fn run_binary_with_manifest<F, G, M, P, I, T>(
    read_factory: &mut F,
    login_factory: &mut G,
    manifest_factory: &mut M,
    present: &mut P,
    args: I,
) -> ProcessOutput
where
    F: RuntimeFactory,
    G: LoginFactory,
    M: crate::runtime::ManifestFactory,
    P: FnMut(&str) -> Result<(), CliError>,
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut weekly =
        crate::runtime::ComposingManifestFactory::new(crate::runtime::ProductionFoundations);
    run_binary_with_public(
        read_factory,
        login_factory,
        manifest_factory,
        &mut weekly,
        present,
        args,
    )
}

/// Injectable routing for both credential-free public commands.
pub fn run_binary_with_public<F, G, M, W, P, I, T>(
    read_factory: &mut F,
    login_factory: &mut G,
    manifest_factory: &mut M,
    weekly_factory: &mut W,
    present: &mut P,
    args: I,
) -> ProcessOutput
where
    F: RuntimeFactory,
    G: LoginFactory,
    M: crate::runtime::ManifestFactory,
    W: crate::runtime::WeeklyResetFactory,
    P: FnMut(&str) -> Result<(), CliError>,
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let cli = match parse_process(args) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    let mode = cli.output_mode();
    if let Command::Weekly(arguments) = &cli.command {
        let level = match arguments.output_level(mode) {
            Ok(level) => level,
            Err(error) => return ProcessOutput::failure(mode, error),
        };
        let result = arguments.character_id().and_then(|selector| {
            crate::runtime::compose_weekly(
                &mut crate::runtime::ProductionFoundations,
                destiny_adapters::manifest::HttpManifestTransport::new()?,
                selector.as_ref(),
            )
        });
        return match result {
            Ok((character, weekly)) => ProcessOutput::success(crate::output::render_weekly_level(
                mode, &character, &weekly, level,
            )),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::WeeklyReset = &cli.command {
        return run_weekly_reset(weekly_factory, mode);
    }
    if let Command::SyncManifest = &cli.command {
        return run_manifest(manifest_factory, mode);
    }
    if let Command::Login(arguments) = &cli.command {
        let mut login =
            match login_factory.build(arguments.mode.authorization_mode(), arguments.callback) {
                Ok(login) => login,
                Err(error) => return ProcessOutput::failure(mode, error),
            };
        return run_login(login.as_mut(), mode, present);
    }
    if let Command::Callback(arguments) = &cli.command {
        return run_callback(&arguments.callback_uri, mode);
    }
    let request = match cli.into_read_request() {
        Ok(request) => request,
        Err(error) => return ProcessOutput::failure(mode, error),
    };
    match read_factory.build() {
        Ok(mut runtime) => {
            let freshness = runtime.freshness();
            dispatch(runtime.service_mut(), request, mode, freshness)
        }
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Runs a public Manifest sync through its credential-free composition.
pub fn run_manifest<M: crate::runtime::ManifestFactory>(
    factory: &mut M,
    mode: OutputMode,
) -> ProcessOutput {
    match factory.sync() {
        Ok(report) => ProcessOutput::success(crate::output::render_manifest(mode, &report)),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Runs a fresh public weekly read through its credential-free composition.
pub fn run_weekly_reset<W: crate::runtime::WeeklyResetFactory>(
    factory: &mut W,
    mode: OutputMode,
) -> ProcessOutput {
    match factory.weekly_reset() {
        Ok(reset) => ProcessOutput::success(crate::output::render_weekly_reset(mode, &reset)),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Runs one composed login attempt and renders it.
///
/// The order is the security-relevant part: the authorization request is started
/// first, the operator instruction the selected mode produced is presented before
/// the callback is awaited (a headless operator cannot act on it otherwise), and
/// only then is exactly one callback awaited. Presentation is fallible: when the
/// instruction cannot be delivered to the operator, the attempt fails closed with
/// a sanitized error and the callback is never awaited, because a wait the
/// operator cannot satisfy would only block on a terminal read forever. Nothing
/// here inspects, stores, or renders credential material: the shared outcome
/// carries expiry metadata only.
pub fn run_login<L, P>(login: &mut L, mode: OutputMode, present: &mut P) -> ProcessOutput
where
    L: LoginOperations + ?Sized,
    P: FnMut(&str) -> Result<(), CliError>,
{
    if let Err(error) = login.begin() {
        return ProcessOutput::failure(mode, CliError::from(error));
    }
    if let Some(instruction) = login.instruction()
        && let Err(error) = present(&render_login_instruction(mode, &instruction))
    {
        return ProcessOutput::failure(mode, error);
    }
    match login.receive_callback() {
        Ok(outcome) => login_result(mode, outcome),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

/// Delivers one operating-system callback to a waiting login process.
///
/// The installed desktop entry invokes the hidden `ghostlink callback <uri>`
/// subcommand with the exact callback URI as its single argument. The value is
/// validated for shape, forwarded once over the per-user rendezvous, and never
/// rendered, logged, or persisted. A desktop `%u` expansion necessarily places
/// the one-use authorization code in this process's argv for the moment of the
/// forward; that exposure is inherent to desktop custom-scheme handling and is
/// not claimed away.
///
/// A successful result means only that the URI was delivered to a waiting login
/// over the rendezvous; it never means the authorization succeeded. The waiting
/// login still owns the cryptographic state comparison, replay rejection, and the
/// code exchange, so it — not this handler — reports the authorization outcome.
/// The handler is silent on both streams in either mode: the operating system
/// runs it with `Terminal=false`, so it has no one to address.
fn run_callback(callback_uri: &str, mode: OutputMode) -> ProcessOutput {
    match callback_delivery(callback_uri) {
        Ok(()) => callback_delivered(),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

/// The result of a handler that delivered exactly one callback.
///
/// Exit success here is deliberately separate from the login's authorization
/// outcome: delivery is the only thing this process observed. Nothing is written
/// to either stream, so no success text can be mistaken for, or rendered as, an
/// authorization result.
fn callback_delivered() -> ProcessOutput {
    ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: SUCCESS_EXIT_CODE,
    }
}

/// Forwards one callback to the waiting login on Linux.
#[cfg(target_os = "linux")]
fn callback_delivery(callback_uri: &str) -> Result<(), destiny_app::AppError> {
    destiny_adapters::oauth::forward_callback(callback_uri)
}

/// A platform without the Linux rendezvous refuses the handler explicitly.
#[cfg(not(target_os = "linux"))]
fn callback_delivery(callback_uri: &str) -> Result<(), destiny_app::AppError> {
    let _ = callback_uri;
    Err(destiny_app::AppError::new(
        ErrorCategory::UnsupportedOperation,
    ))
}

/// Maps one completed authorization outcome to the sanitized process result.
///
/// Every arm is a fixed, value-free outcome from the shared matrix. A callback
/// the human denied fails as denied, a cancellation and every outcome that leaves
/// no authorized session fail as authorization still being required, a rejected or
/// replayed state fails as the authorization state being rejected, an expired
/// window fails as the callback timing out, and a failed or rejected exchange
/// fails as reauthorization being required. No arm can render provider text, a
/// code, a state, or a credential.
fn login_result(mode: OutputMode, outcome: AuthorizationOutcome) -> ProcessOutput {
    match outcome {
        AuthorizationOutcome::AuthorizedSession {
            access,
            refresh_available,
            refresh_expires_at,
        } => ProcessOutput::success(match (refresh_available, refresh_expires_at) {
            (true, Some(expiry)) => crate::output::render_login_renewable(mode, &access, expiry),
            _ => render_login_authorized(mode, &access),
        }),
        AuthorizationOutcome::Authorized(access) => {
            ProcessOutput::success(render_login_authorized(mode, &access))
        }
        AuthorizationOutcome::Denied => ProcessOutput::failure(
            mode,
            CliError::from_category(ErrorCategory::AuthorizationDenied),
        ),
        AuthorizationOutcome::StateMismatch | AuthorizationOutcome::Replay => {
            ProcessOutput::failure(
                mode,
                CliError::from_category(ErrorCategory::CsrfStateMismatch),
            )
        }
        AuthorizationOutcome::Expired => ProcessOutput::failure(
            mode,
            CliError::from_category(ErrorCategory::CallbackTimeout),
        ),
        AuthorizationOutcome::ExchangeFailed | AuthorizationOutcome::ReauthorizationRequired => {
            ProcessOutput::failure(
                mode,
                CliError::from_category(ErrorCategory::ReauthorizationRequired),
            )
        }
        // A cancellation, and every other outcome, leaves the caller without an
        // authorized session, so the honest outcome is that authorization is
        // still required. `Started`, `Authorizing`, `SignedOut` and `LoggedOut`
        // are not part of the callback vocabulary and are handled here as the same
        // sanitized outcome rather than guessed at.
        AuthorizationOutcome::Cancelled
        | AuthorizationOutcome::Started
        | AuthorizationOutcome::Authorizing
        | AuthorizationOutcome::SignedOut
        | AuthorizationOutcome::LoggedOut => ProcessOutput::failure(
            mode,
            CliError::from_category(ErrorCategory::AuthorizationRequired),
        ),
    }
}

/// Writes one instruction line to stdout and flushes it immediately.
///
/// The instruction has to reach the operator before the callback is awaited, so it
/// is written and flushed as its own line instead of being buffered into the final
/// [`ProcessOutput`]. A write or flush failure is reported as a sanitized local
/// I/O failure; the raw operating-system error is never rendered. The caller
/// treats that failure as fatal to the attempt, so the callback is never awaited
/// for an operator who never saw the authorization URL.
fn present_to_stdout(text: &str) -> Result<(), CliError> {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    writeln!(handle, "{text}")
        .and_then(|()| handle.flush())
        .map_err(|_| CliError::from_category(ErrorCategory::LocalStorage))
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
    #![allow(clippy::needless_borrows_for_generic_args)]

    #[test]
    fn renewable_login_dispatch_renders_token_free_summary() {
        for mode in [
            crate::output::OutputMode::Human,
            crate::output::OutputMode::Json,
        ] {
            let output = super::login_result(
                mode,
                destiny_app::AuthorizationOutcome::AuthorizedSession {
                    access: destiny_app::AccessCredentialMetadata {
                        expires_at: destiny_app::Timestamp::new(100),
                    },
                    refresh_available: true,
                    refresh_expires_at: Some(destiny_app::Timestamp::new(900)),
                },
            );
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            assert!(!output.stdout.contains("sentinel"));
            assert!(output.stdout.contains("900"));
        }
    }
    // The read/login test helpers below take `impl AsRef<[&str]>` so a caller can
    // pass either a slice or an array literal. With that flexible test API the
    // `&[...]` borrow at a call site is intentional and reads as the test's
    // argument list, so the generic-argument borrow lint is allowed here.
    use super::{
        ProcessOutput, ReadFreshness, dispatch, parse_process, run, run_binary, run_binary_with,
        run_login,
    };
    use crate::command::{CallbackMode, Cli, CliError, ReadRequest};
    use crate::login::{LoginFactory, LoginInstruction, LoginOperations};
    use crate::output::OutputMode;
    use crate::runtime::{ReadRuntime, RuntimeFactory};
    use destiny_app::auth_ux::AuthorizationMode;
    use destiny_app::{
        AppError, AuthorizationOutcome, Character, CharacterId, CharacterListResult, Dataset,
        DefinitionVersion, ErrorCategory, FreshnessPolicy, FreshnessStatus, InspectItemRequest,
        InventorySearchHit, InventorySearchResult, ItemDefinition, ItemHash, ItemInspection,
        ItemInstance, ItemInstanceId, ItemLocation, QueryMetadata, QueryOperations, Resolution,
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
            slot: None,
            class_type: None,
            plug_category: None,
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

    fn argv<'a>(arguments: impl AsRef<[&'a str]>) -> Vec<String> {
        let mut argv = vec!["ghostlink".to_string()];
        argv.extend(
            arguments
                .as_ref()
                .iter()
                .map(|argument| (*argument).to_string()),
        );
        argv
    }

    /// The call log of one scripted login.
    type LoginCalls = Rc<RefCell<Vec<&'static str>>>;

    /// A login that records its calls and returns one scripted outcome per step.
    #[derive(Clone)]
    struct ScriptedLogin {
        calls: LoginCalls,
        begin: Result<AuthorizationOutcome, AppError>,
        callback: Result<AuthorizationOutcome, AppError>,
        instruction: Option<LoginInstruction>,
    }

    impl ScriptedLogin {
        fn authorized() -> Self {
            Self {
                calls: Rc::new(RefCell::new(Vec::new())),
                begin: Ok(AuthorizationOutcome::Started),
                callback: Ok(AuthorizationOutcome::Authorized(
                    destiny_app::AccessCredentialMetadata {
                        expires_at: Timestamp::new(1_724_000_000),
                    },
                )),
                instruction: None,
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl LoginOperations for ScriptedLogin {
        fn begin(&mut self) -> Result<AuthorizationOutcome, AppError> {
            self.calls.borrow_mut().push("begin");
            self.begin.clone()
        }

        fn receive_callback(&mut self) -> Result<AuthorizationOutcome, AppError> {
            self.calls.borrow_mut().push("receive_callback");
            self.callback.clone()
        }

        fn instruction(&self) -> Option<LoginInstruction> {
            self.calls.borrow_mut().push("instruction");
            self.instruction.clone()
        }
    }

    /// A login factory that records every mode it was asked to compose.
    #[derive(Clone, Default)]
    struct FakeLoginFactory {
        modes: Rc<RefCell<Vec<AuthorizationMode>>>,
        callbacks: Rc<RefCell<Vec<CallbackMode>>>,
        failure: Option<CliError>,
        script: Option<ScriptedLogin>,
    }

    impl FakeLoginFactory {
        fn modes(&self) -> Vec<AuthorizationMode> {
            self.modes.borrow().clone()
        }

        fn callbacks(&self) -> Vec<CallbackMode> {
            self.callbacks.borrow().clone()
        }
    }

    impl LoginFactory for FakeLoginFactory {
        fn build(
            &mut self,
            mode: AuthorizationMode,
            callback: CallbackMode,
        ) -> Result<Box<dyn LoginOperations>, CliError> {
            self.modes.borrow_mut().push(mode);
            self.callbacks.borrow_mut().push(callback);
            match (self.failure, self.script.clone()) {
                (Some(error), _) => Err(error),
                (None, Some(script)) => Ok(Box::new(script)),
                (None, None) => Ok(Box::new(ScriptedLogin::authorized())),
            }
        }
    }

    /// Runs one read command line through the boundary with an observable login
    /// factory, and proves the login seam was not touched by a read.
    fn run_read_boundary<'a, F: RuntimeFactory>(
        factory: &mut F,
        arguments: impl AsRef<[&'a str]>,
    ) -> (ProcessOutput, FakeLoginFactory) {
        let mut login = FakeLoginFactory::default();
        let mut presented: Vec<String> = Vec::new();
        let output = run_binary_with(
            factory,
            &mut login,
            &mut |text: &str| {
                presented.push(text.to_string());
                Ok(())
            },
            argv(arguments),
        );
        assert!(
            presented.is_empty(),
            "a read command presents no login instruction"
        );
        assert!(
            login.modes().is_empty(),
            "a read command must not compose a login"
        );
        (output, login)
    }

    #[test]
    fn manifest_routing_never_builds_read_or_login_and_sanitizes_failures() {
        struct Manifest {
            calls: usize,
        }
        impl crate::runtime::ManifestFactory for Manifest {
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                self.calls += 1;
                Err(CliError::from_category(ErrorCategory::ProviderUnavailable))
            }
        }
        let mut read = FakeFactory::new(RecordingService::new());
        let mut login = FakeLoginFactory::default();
        let mut manifest = Manifest { calls: 0 };
        for arguments in [
            vec!["sync-manifest", "--unknown"],
            vec!["sync-manifest", "--help"],
        ] {
            super::run_binary_with_manifest(
                &mut read,
                &mut login,
                &mut manifest,
                &mut |_| panic!("sync must not present login"),
                argv(arguments),
            );
        }
        assert_eq!(manifest.calls, 0);
        let output = super::run_binary_with_manifest(
            &mut read,
            &mut login,
            &mut manifest,
            &mut |_| panic!("sync must not present login"),
            argv(["--json", "sync-manifest"]),
        );
        assert_eq!(manifest.calls, 1);
        assert_eq!(read.build_count(), 0);
        assert!(login.modes().is_empty());
        assert_eq!(output.exit_code, 21);
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"provider_unavailable\",\"message\":\"the provider is unavailable\"}}\n"
        );
    }

    #[test]
    fn weekly_routing_never_builds_read_login_or_manifest() {
        struct Public;
        impl crate::runtime::ManifestFactory for Public {
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                panic!("weekly read must not sync")
            }
        }
        struct Weekly(usize);
        impl crate::runtime::WeeklyResetFactory for Weekly {
            fn weekly_reset(&mut self) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
                self.0 += 1;
                Err(CliError::from_category(
                    ErrorCategory::DefinitionsUnavailable,
                ))
            }
        }
        let mut read = FakeFactory::new(RecordingService::new());
        let mut login = FakeLoginFactory::default();
        let mut weekly = Weekly(0);
        for arguments in [
            vec!["weekly-reset", "--unknown"],
            vec!["weekly-reset", "--help"],
        ] {
            super::run_binary_with_public(
                &mut read,
                &mut login,
                &mut Public,
                &mut weekly,
                &mut |_| panic!("weekly read must not present login"),
                argv(arguments),
            );
        }
        assert_eq!(weekly.0, 0);
        for flag in ["--details", "--debug"] {
            let output = super::run_binary_with_public(
                &mut read,
                &mut login,
                &mut Public,
                &mut weekly,
                &mut |_| panic!("weekly must not present login"),
                argv(["weekly", flag, "--json"]),
            );
            assert_eq!(output.exit_code, 2);
            assert_eq!(
                output.stderr,
                "{\"error\":{\"code\":\"invalid_request\",\"message\":\"the request arguments are invalid\"}}\n"
            );
        }
        let output = super::run_binary_with_public(
            &mut read,
            &mut login,
            &mut Public,
            &mut weekly,
            &mut |_| panic!("weekly read must not present login"),
            argv(["weekly-reset", "--json"]),
        );
        assert_eq!(weekly.0, 1);
        assert_eq!(read.build_count(), 0);
        assert!(login.modes().is_empty());
        assert_eq!(output.exit_code, 33);
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"definitions_unavailable\",\"message\":\"definitions not loaded; run ghostlink sync-manifest\"}}\n"
        );
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
                element: None,
                slot: None,
                class_type: None,
                limit: 5,
            },
            instances: Some(destiny_app::InstanceSearchQuery {
                item_hashes: destiny_app::ItemHashFilter::AnyOf(vec![ItemHash::new(1)]),
                perk_hash: None,
                character_id: None,
                definitions: Some(Box::new(destiny_app::DefinitionSearchQuery {
                    text: Some(destiny_app::SearchTerm::new("gjallar").unwrap()),
                    item_type: None,
                    tags: Vec::new(),
                    element: None,
                    slot: None,
                    class_type: None,
                    limit: 5,
                })),
                min_power: None,
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
        let service = RecordingService::new();
        let mut target: Box<dyn QueryOperations> = Box::new(service.clone());
        let output = dispatch(
            target.as_mut(),
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
        let (output, _) = run_read_boundary(&mut factory, &["inspect-item", "--unknown"]);
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");

        // A parsed but semantically invalid request.
        let (output, _) = run_read_boundary(&mut factory, &["search-inventory", "--limit", "0"]);
        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");

        // A missing required argument is rejected before any runtime work too.
        let (output, _) = run_read_boundary(&mut factory, &["inspect-item"]);
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

        let (output, _) = run_read_boundary(&mut factory, &["list-characters"]);

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

        let (output, _) = run_read_boundary(&mut factory, &["list-characters"]);
        assert_eq!(output.exit_code, 50);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: credential storage is unavailable\n");
        assert_eq!(factory.build_count(), 1);
        assert!(service.recorded().is_empty());

        let mut factory = FakeFactory::new(service.clone());
        factory.failure = Some(CliError::from_category(ErrorCategory::ConfigurationError));
        let (output, _) = run_read_boundary(&mut factory, &["--json", "list-characters"]);
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

    /// Runs one login command line through the boundary, with an observable login
    /// factory and a read factory that must never be built.
    fn run_login_boundary(
        login: &mut FakeLoginFactory,
        arguments: &[&str],
    ) -> (ProcessOutput, Vec<String>) {
        let mut read_factory = FakeFactory::new(RecordingService::new());
        let mut presented: Vec<String> = Vec::new();
        let output = run_binary_with(
            &mut read_factory,
            login,
            &mut |text: &str| {
                presented.push(text.to_string());
                Ok(())
            },
            argv(arguments),
        );
        assert_eq!(
            read_factory.build_count(),
            0,
            "a login command must never build the read runtime"
        );
        (output, presented)
    }

    #[test]
    fn login_routes_to_the_login_seam_and_never_to_the_read_runtime() {
        let script = ScriptedLogin::authorized();
        let recorded = script.clone();
        let mut login = FakeLoginFactory {
            script: Some(script),
            ..FakeLoginFactory::default()
        };

        let (output, presented) = run_login_boundary(&mut login, ["login"].as_slice());

        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        assert!(output.stdout.contains("authorized"));
        assert!(presented.is_empty());
        assert_eq!(login.modes(), [AuthorizationMode::Auto]);
        // The authorization request starts, the instruction is read, and only
        // then is the callback awaited.
        assert_eq!(
            recorded.calls(),
            ["begin", "instruction", "receive_callback"],
            "the login order is fixed"
        );
    }

    #[test]
    fn the_requested_login_mode_reaches_the_composition_unresolved() {
        for (flag, expected) in [
            (None, AuthorizationMode::Auto),
            (Some("browser"), AuthorizationMode::Browser),
            (Some("headless"), AuthorizationMode::Headless),
        ] {
            let mut login = FakeLoginFactory::default();
            let arguments: Vec<&str> = match flag {
                Some(value) => vec!["login", "--mode", value],
                None => vec!["login"],
            };

            let (output, _) = run_login_boundary(&mut login, arguments.as_slice());

            assert_eq!(output.exit_code, 0, "flag: {flag:?}");
            assert_eq!(login.modes(), [expected], "flag: {flag:?}");
        }
    }

    #[test]
    fn the_requested_callback_strategy_reaches_the_composition() {
        for (flag, expected) in [
            (None, CallbackMode::Paste),
            (Some("paste"), CallbackMode::Paste),
            (Some("handler"), CallbackMode::Handler),
        ] {
            let mut login = FakeLoginFactory::default();
            let arguments: Vec<&str> = match flag {
                Some(value) => vec!["login", "--callback", value],
                None => vec!["login"],
            };

            let (output, _) = run_login_boundary(&mut login, arguments.as_slice());

            assert_eq!(output.exit_code, 0, "flag: {flag:?}");
            assert_eq!(login.callbacks(), [expected], "flag: {flag:?}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_hidden_callback_handler_routes_to_the_forwarder_and_is_silent() {
        // A malformed argument is rejected by the forwarder's shape check before
        // any rendezvous is touched, which proves the callback route and keeps the
        // test independent of the host's runtime directory.
        let mut read_factory = FakeFactory::new(RecordingService::new());
        let mut login = FakeLoginFactory::default();
        let mut presented: Vec<String> = Vec::new();
        let output = run_binary_with(
            &mut read_factory,
            &mut login,
            &mut |text: &str| {
                presented.push(text.to_string());
                Ok(())
            },
            argv(
                [
                    "callback",
                    "https://evil.test/oauth/callback?state=s&code=c",
                ]
                .as_slice(),
            ),
        );

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: authorization state was rejected\n");
        assert!(presented.is_empty());
        assert_eq!(read_factory.build_count(), 0);
        assert!(login.modes().is_empty());
        assert!(login.callbacks().is_empty());
    }

    #[test]
    fn an_invalid_login_command_never_composes_a_login() {
        let mut login = FakeLoginFactory::default();

        let (output, presented) =
            run_login_boundary(&mut login, ["login", "--mode", "bogus"].as_slice());

        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the request arguments are invalid\n");
        assert!(presented.is_empty());
        assert!(login.modes().is_empty());
    }

    #[test]
    fn a_login_composition_failure_is_sanitized_in_both_output_modes() {
        let mut login = FakeLoginFactory {
            failure: Some(CliError::from_category(ErrorCategory::ConfigurationError)),
            ..FakeLoginFactory::default()
        };

        let (output, _) = run_login_boundary(&mut login, ["login"].as_slice());
        assert_eq!(output.exit_code, 52);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "error: required runtime configuration is missing or invalid\n"
        );
        assert_eq!(login.modes(), [AuthorizationMode::Auto]);

        let mut login = FakeLoginFactory {
            failure: Some(CliError::from_category(
                ErrorCategory::CredentialStoreUnavailable,
            )),
            ..FakeLoginFactory::default()
        };
        let (output, _) = run_login_boundary(&mut login, ["--json", "login"].as_slice());
        assert_eq!(output.exit_code, 50);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "{\"error\":{\"code\":\"credential_store_unavailable\",\"message\":\"credential storage is unavailable\"}}\n"
        );
    }

    #[test]
    fn login_outcomes_map_to_the_stable_sanitized_matrix() {
        let cases = [
            (AuthorizationOutcome::Denied, "authorization_denied", 10),
            (
                AuthorizationOutcome::Cancelled,
                "authorization_required",
                10,
            ),
            (
                AuthorizationOutcome::StateMismatch,
                "authorization_state",
                10,
            ),
            (AuthorizationOutcome::Replay, "authorization_state", 10),
            (AuthorizationOutcome::Expired, "provider_unavailable", 21),
            (AuthorizationOutcome::Started, "authorization_required", 10),
            (
                AuthorizationOutcome::Authorizing,
                "authorization_required",
                10,
            ),
            (
                AuthorizationOutcome::SignedOut,
                "authorization_required",
                10,
            ),
            (
                AuthorizationOutcome::LoggedOut,
                "authorization_required",
                10,
            ),
            (
                AuthorizationOutcome::ExchangeFailed,
                "reauthorization_required",
                11,
            ),
            (
                AuthorizationOutcome::ReauthorizationRequired,
                "reauthorization_required",
                11,
            ),
        ];

        for (outcome, code, exit_code) in cases {
            let mut script = ScriptedLogin::authorized();
            script.callback = Ok(outcome.clone());
            let mut login = FakeLoginFactory {
                script: Some(script),
                ..FakeLoginFactory::default()
            };

            let (output, _) = run_login_boundary(&mut login, ["--json", "login"].as_slice());

            assert_eq!(output.exit_code, exit_code, "outcome: {outcome:?}");
            assert!(output.stdout.is_empty(), "outcome: {outcome:?}");
            let parsed: serde_json::Value =
                serde_json::from_str(output.stderr.trim()).expect("one bounded JSON object");
            assert_eq!(parsed["error"]["code"], code, "outcome: {outcome:?}");
            assert!(
                !output.stderr.contains("sentinel"),
                "a failure never echoes a supplied value"
            );
        }
    }

    #[test]
    fn a_login_step_failure_is_reported_as_a_typed_sanitized_outcome() {
        let mut script = ScriptedLogin::authorized();
        script.begin = Err(AppError::new(ErrorCategory::ProviderUnavailable));
        let mut login = FakeLoginFactory {
            script: Some(script),
            ..FakeLoginFactory::default()
        };
        let (output, presented) = run_login_boundary(&mut login, ["login"].as_slice());
        assert_eq!(output.exit_code, 21);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the provider is unavailable\n");
        // A failed start is never followed by a callback wait.
        assert!(presented.is_empty());

        let mut script = ScriptedLogin::authorized();
        script.callback = Err(AppError::new(ErrorCategory::CallbackTimeout));
        let mut login = FakeLoginFactory {
            script: Some(script.clone()),
            ..FakeLoginFactory::default()
        };
        let (output, _) = run_login_boundary(&mut login, ["login"].as_slice());
        assert_eq!(output.exit_code, 21);
        assert_eq!(
            output.stderr,
            "error: the authorization callback timed out\n"
        );
        assert_eq!(script.calls(), ["begin", "instruction", "receive_callback"]);
    }

    #[test]
    fn a_headless_login_presents_its_instruction_before_the_outcome() {
        let mut script = ScriptedLogin::authorized();
        script.instruction = Some(LoginInstruction {
            authorization_url: "https://www.bungie.net/en/oauth/authorize?state=abc".to_string(),
            callback_mode: CallbackMode::Paste,
        });
        let recorded = script.clone();
        let mut login = FakeLoginFactory {
            script: Some(script),
            ..FakeLoginFactory::default()
        };

        let (output, presented) =
            run_login_boundary(&mut login, ["login", "--mode", "headless"].as_slice());

        assert_eq!(output.exit_code, 0);
        assert_eq!(presented.len(), 1, "one instruction block is presented");
        assert!(presented[0].contains("authorization required"));
        assert!(presented[0].contains("state=abc"));
        assert!(presented[0].contains("paste the callback url"));
        assert!(output.stdout.contains("authorized"));
        assert_eq!(login.modes(), [AuthorizationMode::Headless]);
        assert_eq!(
            recorded.calls(),
            ["begin", "instruction", "receive_callback"]
        );
    }

    #[test]
    fn run_login_accepts_a_trait_object_and_orders_its_calls() {
        let script = ScriptedLogin::authorized();
        let recorded = script.clone();
        let mut login: Box<dyn LoginOperations> = Box::new(script);
        let mut presented: Vec<String> = Vec::new();

        let output = run_login(login.as_mut(), OutputMode::Human, &mut |text: &str| {
            presented.push(text.to_string());
            Ok(())
        });

        assert_eq!(output.exit_code, 0);
        assert!(presented.is_empty());
        assert_eq!(
            recorded.calls(),
            ["begin", "instruction", "receive_callback"]
        );
    }

    #[test]
    fn a_failed_instruction_presentation_fails_closed_before_the_callback() {
        // A headless operator cannot authorize without the URL, so a presentation
        // failure must never fall through to a callback wait that can never be
        // satisfied. The attempt fails with a sanitized error and the terminal read
        // is never entered.
        let mut script = ScriptedLogin::authorized();
        script.instruction = Some(LoginInstruction {
            authorization_url: "https://www.bungie.net/en/oauth/authorize?state=abc".to_string(),
            callback_mode: CallbackMode::Paste,
        });
        let recorded = script.clone();
        let mut login: Box<dyn LoginOperations> = Box::new(script);

        let output = run_login(login.as_mut(), OutputMode::Human, &mut |_: &str| {
            Err(CliError::from_category(ErrorCategory::LocalStorage))
        });

        assert_eq!(output.exit_code, 51);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: local storage failed\n");
        assert_eq!(
            recorded.calls(),
            ["begin", "instruction"],
            "a failed presentation is never followed by a callback wait"
        );
    }

    #[test]
    fn the_read_entries_refuse_a_login_command_rather_than_guessing_at_a_read() {
        // A read entry point handed a login command line has no read to run, so it
        // reports the stable unsupported-operation outcome instead of an empty or
        // invented read. The process boundary never routes login here.
        let mut service = RecordingService::new();
        let output = run(&mut service, argv(["login"].as_slice()), freshness());
        assert_eq!(output.exit_code, 40);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the operation is unsupported\n");
        assert!(service.recorded().is_empty());
    }
}
