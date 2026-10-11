//! The thin command runner and the sanitized process boundary.
//!
//! The runner validates one parsed command line, dispatches reads
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
    render_login_instruction, render_search, render_subclass_options, render_subclasses,
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
    pub(crate) fn success(body: String) -> Self {
        Self {
            stdout: format!("{body}\n"),
            stderr: String::new(),
            exit_code: SUCCESS_EXIT_CODE,
        }
    }

    /// Builds a sanitized failure written to stderr.
    pub(crate) fn failure(mode: OutputMode, error: CliError) -> Self {
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
        Ok(cli) if matches!(&cli.command, crate::command::Command::SearchInventory(args) if args.filters) =>
        {
            let mut text = String::from(
                "DIM syntax: whitespace = implicit AND; explicit and > or > whitespace (DIM precedence).\nNegate with - or not; group with parentheses; quote phrases. Numeric operators: = < <= > >=.\nQuery and structured flags combine via AND; empty query = all owned.\n\n",
            );
            for filter in destiny_app::search_query::list_filters() {
                use std::fmt::Write;
                let _ = writeln!(
                    text,
                    "{} [{:?}] — {}\n  Example: {}",
                    filter.keys.join(" / "),
                    filter.format,
                    filter.description,
                    filter.examples.join("; ")
                );
            }
            Err(ProcessOutput {
                exit_code: 0,
                stdout: text,
                stderr: String::new(),
            })
        }
        Ok(cli) => Ok(cli),
        Err(error) => Err(parse_failure(json_mode(&args), &error)),
    }
}

/// Validates one parsed command line and dispatches it.
/// Character-filtered searches first read characters to resolve the selector.
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

/// Dispatches one already-validated request through the shared trait.
/// Character-filtered searches resolve the selector before the search read.
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
    let search_detail = matches!(&request, ReadRequest::SearchInventoryDetail(_));
    let rendered: Result<String, CliError> = match request {
        // Live sibling services are composed separately at the binary boundary.
        ReadRequest::Artifact { .. } | ReadRequest::Loadouts { .. } => {
            Err(CliError::from_category(ErrorCategory::UnsupportedOperation))
        }
        ReadRequest::SubclassOptions {
            character_id,
            element,
            locked,
        } => service
            .subclass_options(
                character_id.as_ref(),
                element,
                freshness.policy,
                freshness.now,
            )
            .map(|result| render_subclass_options(mode, &result, locked))
            .map_err(CliError::from),
        ReadRequest::Postmaster(character_id) => service
            .postmaster(character_id.as_ref(), freshness.policy, freshness.now)
            .map(|result| crate::postmaster_output::render_postmaster(mode, &result))
            .map_err(CliError::from),
        ReadRequest::Subclass(character_id) => service
            .character_subclasses(character_id.as_ref(), freshness.policy, freshness.now)
            .map(|result| render_subclasses(mode, &result))
            .map_err(CliError::from),
        ReadRequest::ListCharacters => service
            .list_characters(freshness.policy, freshness.now)
            .map(|result| render_characters(mode, &result))
            .map_err(CliError::from),
        ReadRequest::SearchInventory(mut request)
        | ReadRequest::SearchInventoryDetail(mut request) => {
            let selection = match request
                .instances
                .as_mut()
                .and_then(|query| query.character_id.as_mut())
            {
                Some(selector) => service
                    .list_characters(freshness.policy, freshness.now)
                    .map_err(CliError::from)
                    .and_then(|result| {
                        crate::runtime::select_weekly_character(&result.characters, Some(selector))
                    })
                    .map(|character| *selector = character.id),
                None => Ok(()),
            };
            selection.and_then(|()| {
                service
                    .search_inventory(&request, freshness.policy, freshness.now)
                    .map(|result| {
                        if search_detail {
                            crate::output::render_search_detail(mode, &result)
                        } else {
                            render_search(mode, &result)
                        }
                    })
                    .map_err(CliError::from)
            })
        }
        ReadRequest::InspectItemByName(name) => {
            let result = match service.owned_name_hits(&name, freshness.policy, freshness.now) {
                Ok(result) => result,
                Err(error) => return ProcessOutput::failure(mode, CliError::from(error)),
            };
            match result.hits.as_slice() {
                [] => return ProcessOutput::failure(mode, CliError::item_not_found()),
                [hit] => service
                    .inspect_item(
                        &destiny_app::InspectItemRequest {
                            instance_id: hit.instance.instance_id.clone(),
                        },
                        freshness.policy,
                        freshness.now,
                    )
                    .map_err(CliError::from)
                    .and_then(inspection_outcome)
                    .map(|inspection| render_inspection(mode, &inspection)),
                _ => {
                    let value: serde_json::Value =
                        serde_json::from_str(&render_search(OutputMode::Json, &result))
                            .expect("local search projection");
                    let candidates = value["hits"].as_array().expect("search hits").clone();
                    let error = destiny_output::search::ambiguous_items(candidates.clone());
                    let stderr = match mode {
                        OutputMode::Json => serde_json::json!({"error": error}).to_string(),
                        OutputMode::Human => format!(
                            "error: {}\n{}",
                            error["message"].as_str().expect("error message"),
                            candidates
                                .iter()
                                .map(crate::output::search_output::compact_line)
                                .collect::<Vec<_>>()
                                .join("\n")
                        ),
                    };
                    return ProcessOutput {
                        stdout: String::new(),
                        stderr: format!("{stderr}\n"),
                        exit_code: CliError::from_category(ErrorCategory::AmbiguousMatch)
                            .exit_code(),
                    };
                }
            }
        }
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

/// Dispatches a live loadouts read through its sibling service contract.
pub fn dispatch_loadouts<S: destiny_app::LoadoutOperations + ?Sized>(
    service: &mut S,
    selector: Option<&destiny_app::CharacterId>,
    detail: bool,
    mode: OutputMode,
    freshness: ReadFreshness,
) -> ProcessOutput {
    match service.loadouts(selector, freshness.policy, freshness.now) {
        Ok(result) => ProcessOutput::success(crate::output::render_loadouts(mode, &result, detail)),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

/// Dispatches one saved-loadout detail read with the same freshness and selector policy.
pub fn dispatch_loadout<S: destiny_app::LoadoutOperations + ?Sized>(
    service: &mut S,
    character: Option<&destiny_app::CharacterId>,
    selector: &destiny_app::LoadoutSelector,
    mode: OutputMode,
    freshness: ReadFreshness,
) -> ProcessOutput {
    match service.loadouts(character, freshness.policy, freshness.now) {
        Ok(result) => render_loadouts_selection(mode, &result, true, Some(selector)),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

fn render_loadouts_selection(
    mode: OutputMode,
    result: &destiny_app::LoadoutsResult,
    detail: bool,
    selector: Option<&destiny_app::LoadoutSelector>,
) -> ProcessOutput {
    let Some(selector) = selector else {
        return ProcessOutput::success(crate::output::render_loadouts(mode, result, detail));
    };
    match destiny_app::loadouts::select_loadout(result, selector) {
        Ok(selected) => ProcessOutput::success(crate::output::render_loadout(mode, &selected)),
        Err(error) => {
            let output = destiny_output::loadouts::selector_error(error);
            let body = if mode == OutputMode::Json {
                output.value.to_string()
            } else {
                output
                    .render_text()
                    .replacen("Apply loadout:", "Loadout:", 1)
            };
            ProcessOutput {
                stdout: String::new(),
                stderr: format!("{body}\n"),
                exit_code: output.exit_code,
            }
        }
    }
}

/// Dispatches a live artifact read through its sibling service contract.
pub fn dispatch_artifact<S: destiny_app::ArtifactOperations + ?Sized>(
    service: &mut S,
    selector: Option<&destiny_app::CharacterId>,
    mode: OutputMode,
    freshness: ReadFreshness,
) -> ProcessOutput {
    match service.artifact(selector, freshness.policy, freshness.now) {
        Ok(result) => ProcessOutput::success(crate::output::render_artifact(mode, &result)),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
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
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let cli = match parse_process(args.iter().cloned()) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    if let Command::Update(arguments) = &cli.command {
        return crate::update::production(arguments, cli.output_mode());
    }
    if let Command::Uninstall(arguments) = &cli.command {
        return crate::uninstall::production(arguments, cli.output_mode());
    }
    let mut read_factory = crate::runtime::production_runtime_factory();
    let mut login_factory = crate::login::ProductionLoginFactory::new();
    let mut manifest =
        crate::runtime::ComposingManifestFactory::automatic(crate::runtime::ProductionFoundations);
    run_binary_with_manifest(
        &mut read_factory,
        &mut login_factory,
        &mut manifest,
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
/// request can never trigger a configuration read, a loopback bind, a credential
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

/// Injectable routing for Manifest sync and authenticated weekly reset.
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
    if let Command::Update(arguments) = &cli.command {
        return crate::update::production(arguments, mode);
    }
    if let Command::Uninstall(arguments) = &cli.command {
        return crate::uninstall::production(arguments, mode);
    }
    if matches!(cli.command, Command::Help | Command::Setup) {
        use crate::setup::MarkerStore;
        use std::io::IsTerminal;
        let explicit = matches!(cli.command, Command::Setup);
        let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        // Check usage before even resolving the data path.
        if explicit && (!tty || mode == OutputMode::Json) {
            return crate::setup::usage_error(mode);
        }
        struct UnresolvedMarker;
        impl MarkerStore for UnresolvedMarker {
            fn complete(&self) -> bool {
                false
            }
            fn write(&mut self) -> Result<(), std::io::Error> {
                Err(std::io::Error::other("unresolved data path"))
            }
        }
        let path = destiny_app::setup::marker_path();
        let mut marker: Box<dyn MarkerStore> = match path {
            Some(path) => Box::new(crate::setup::FileMarker(path)),
            None => Box::new(UnresolvedMarker),
        };
        return crate::setup::entry(explicit, tty, mode, marker.as_mut(), &mut |marker| {
            crate::setup::production(marker, &mut || {
                run_binary_with_manifest(
                    read_factory,
                    login_factory,
                    manifest_factory,
                    present,
                    ["ghostlink", "login", "--callback", "auto"],
                )
            })
        });
    }
    if let Command::EquipItems(arguments) = &cli.command {
        return match crate::runtime::compose_equip_items(arguments) {
            Ok(output) => {
                let body = match mode {
                    OutputMode::Human => output.render_text(),
                    OutputMode::Json => output.value.to_string(),
                };
                if output.exit_code == 0 {
                    ProcessOutput::success(body)
                } else {
                    ProcessOutput {
                        stdout: String::new(),
                        stderr: format!("{body}\n"),
                        exit_code: output.exit_code,
                    }
                }
            }
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::Build(arguments) = &cli.command {
        return match crate::builds::compose(arguments) {
            Ok(output) => {
                // Export is always raw DIM JSON, including --json (docs/cli.md).
                let body = if let Some(export) = output.export {
                    export
                } else if mode == OutputMode::Json {
                    output.value.to_string()
                } else {
                    output.render_text()
                };
                if output.exit_code == 0 {
                    ProcessOutput::success(body)
                } else {
                    ProcessOutput {
                        stdout: String::new(),
                        stderr: format!("{body}\n"),
                        exit_code: output.exit_code,
                    }
                }
            }
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::ApplyLoadout(arguments) = &cli.command {
        return match crate::runtime::compose_apply_loadout(arguments) {
            Ok(output) => {
                let body = match mode {
                    OutputMode::Human => output.render_text(),
                    OutputMode::Json => output.value.to_string(),
                };
                if output.exit_code == 0 {
                    ProcessOutput::success(body)
                } else {
                    ProcessOutput {
                        stdout: String::new(),
                        stderr: format!("{body}\n"),
                        exit_code: output.exit_code,
                    }
                }
            }
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::PullStack(arguments) = &cli.command {
        return dispatch_pull_stack(arguments, mode, crate::runtime::compose_pull_stack);
    }
    if let Command::Pull(arguments) = &cli.command {
        // One composition only: never retry/remint a pull after any outcome.
        return dispatch_pull(arguments, mode, crate::runtime::compose_pull);
    }
    if let Command::Lock(arguments) | Command::Unlock(arguments) = &cli.command {
        if let Err(error) = arguments.validate() {
            return ProcessOutput::failure(mode, error);
        }
        // Never recompose after dispatch uncertainty or acquire definitions by retry.
        return match crate::runtime::compose_lock(
            arguments,
            matches!(cli.command, Command::Lock(_)),
        ) {
            Ok(output) => lock_process_output(mode, &output),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::Move(arguments) = &cli.command {
        if let Err(error) = arguments.validate() {
            return ProcessOutput::failure(mode, error);
        }
        // Never recompose a mutation after an error: dispatch uncertainty is
        // authoritative. Definition acquisition remains an explicit sync.
        return match crate::runtime::compose_move(arguments) {
            Ok(output) => move_process_output(mode, &output),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::Equip(arguments) = &cli.command {
        if let Err(error) = arguments.validate() {
            return ProcessOutput::failure(mode, error);
        }
        return match with_definition_retry(manifest_factory, || {
            crate::runtime::compose_equip(arguments)
        }) {
            Ok(output) => equip_process_output(mode, &output),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::Weekly(arguments) = &cli.command {
        let level = match arguments.output_level(mode) {
            Ok(level) => level,
            Err(error) => return ProcessOutput::failure(mode, error),
        };
        let selector = match arguments.character_id() {
            Ok(selector) => selector,
            Err(error) => return ProcessOutput::failure(mode, error),
        };
        let result = with_definition_retry(manifest_factory, || {
            destiny_adapters::manifest::HttpManifestTransport::new()
                .map_err(CliError::from)
                .and_then(|transport| {
                    crate::runtime::compose_weekly(
                        &mut crate::runtime::ProductionFoundations,
                        transport,
                        selector.as_ref(),
                    )
                })
        });
        return match result {
            Ok((character, weekly)) => ProcessOutput::success(crate::output::render_weekly_level(
                mode, &character, &weekly, level,
            )),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::WeeklyReset(arguments) = &cli.command {
        let requested = arguments
            .date
            .as_deref()
            .map(|date| destiny_adapters::calendar::instant_for_date(date, &chrono::Local))
            .transpose();
        return match requested {
            Ok(requested) => run_weekly_reset(weekly_factory, mode, requested),
            Err(_) => ProcessOutput::failure(mode, CliError::invalid_request()),
        };
    }
    if let Command::Sync = &cli.command {
        return match build_with_definitions(read_factory, manifest_factory) {
            Ok(mut runtime) => {
                let freshness = runtime.freshness();
                run_profile_sync(runtime.service_mut(), mode, freshness)
            }
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let Command::SyncManifest = &cli.command {
        return run_manifest(manifest_factory, mode);
    }
    if let Command::Agents(arguments) = &cli.command {
        return crate::agents::production(&arguments.command, mode);
    }
    if let Command::Handler(arguments) = &cli.command {
        return crate::handler::run_handler(
            crate::handler::production_registrar().as_mut(),
            arguments.command,
            mode,
        );
    }
    if let Command::Login(arguments) = &cli.command {
        if let Some(action) = &arguments.action {
            let mut output = crate::headless_login::run_production(action, mode);
            if output.exit_code == 0
                && matches!(action, crate::command::LoginAction::Complete(_))
                && ensure_definitions(manifest_factory).is_err()
            {
                output.stderr.push_str(
                    "Manifest sync failed; run ghostlink sync-manifest before reading data.\n",
                );
            }
            return output;
        }
        let mut login = match login_factory.build_with_output(
            arguments.mode.authorization_mode(),
            arguments.callback,
            mode,
        ) {
            Ok(login) => login,
            Err(error) => return ProcessOutput::failure(mode, error),
        };
        let mut output = run_login(login.as_mut(), mode, present);
        if output.exit_code == 0 {
            match ensure_definitions(manifest_factory) {
                Ok(_) => {}
                Err(_) => output.stderr.push_str(
                    "Manifest sync failed; run ghostlink sync-manifest before reading data.\n",
                ),
            }
        }
        return output;
    }
    if let Command::Callback(arguments) = &cli.command {
        return run_callback(&arguments.callback_uri, mode);
    }
    let request = match cli.into_read_request() {
        Ok(request) => request,
        Err(error) => return ProcessOutput::failure(mode, error),
    };
    if let ReadRequest::Loadouts {
        character_id,
        detail,
        loadout,
    } = request
    {
        let result = with_definition_retry(manifest_factory, || {
            crate::runtime::compose_loadouts(
                &mut crate::runtime::ProductionFoundations,
                character_id.as_ref(),
            )
        });
        return match result {
            Ok(result) => render_loadouts_selection(mode, &result, detail, loadout.as_ref()),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    if let ReadRequest::Artifact { character_id } = request {
        let result = with_definition_retry(manifest_factory, || {
            crate::runtime::compose_artifact(
                &mut crate::runtime::ProductionFoundations,
                character_id.as_ref(),
            )
        });
        return match result {
            Ok(result) => ProcessOutput::success(crate::output::render_artifact(mode, &result)),
            Err(error) => ProcessOutput::failure(mode, error),
        };
    }
    match build_with_definitions(read_factory, manifest_factory) {
        Ok(mut runtime) => {
            let freshness = runtime.freshness();
            dispatch(runtime.service_mut(), request, mode, freshness)
        }
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

fn build_with_definitions<F: RuntimeFactory, M: crate::runtime::ManifestFactory>(
    read: &mut F,
    manifest: &mut M,
) -> Result<crate::runtime::ReadRuntime<F::Service>, CliError> {
    // Preserve authorization/configuration failures before a public download.
    match read.build() {
        Ok(runtime) => {
            ensure_definitions(manifest)?;
            Ok(runtime)
        }
        Err(error) if error == CliError::from_category(ErrorCategory::DefinitionsUnavailable) => {
            ensure_definitions(manifest)?;
            read.build()
        }
        Err(error) => Err(error),
    }
}

fn with_definition_retry<M: crate::runtime::ManifestFactory, T>(
    manifest: &mut M,
    mut operation: impl FnMut() -> Result<T, CliError>,
) -> Result<T, CliError> {
    match operation() {
        Err(error) if error == CliError::from_category(ErrorCategory::DefinitionsUnavailable) => {
            ensure_definitions(manifest)?;
            operation()
        }
        result => result,
    }
}

fn ensure_definitions<M: crate::runtime::ManifestFactory>(factory: &mut M) -> Result<(), CliError> {
    if factory.definitions_missing()? {
        eprintln!("Definitions not loaded; syncing manifest...");
        factory.sync()?;
    }
    Ok(())
}

/// Injectable parser-to-runner stack boundary; invalid inputs never acquire I/O.
pub fn run_pull_stack_with<I, T, F>(args: I, operation: F) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
    F: FnOnce(
        crate::command::StackPullSelection,
    ) -> Result<crate::pull_stack_output::StackPullOutput, CliError>,
{
    let cli = match parse_process(args) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    let mode = cli.output_mode();
    match cli.command {
        Command::PullStack(arguments) => dispatch_pull_stack(&arguments, mode, operation),
        _ => ProcessOutput::failure(
            mode,
            CliError::from_category(ErrorCategory::UnsupportedOperation),
        ),
    }
}
fn dispatch_pull_stack<F>(
    arguments: &crate::command::PullStackArgs,
    mode: OutputMode,
    operation: F,
) -> ProcessOutput
where
    F: FnOnce(
        crate::command::StackPullSelection,
    ) -> Result<crate::pull_stack_output::StackPullOutput, CliError>,
{
    let selection = match arguments.selection() {
        Ok(selection) => selection,
        Err(error) => return ProcessOutput::failure(mode, error),
    };
    match operation(selection) {
        Ok(output) => {
            let body = match mode {
                OutputMode::Human => output.render_text(),
                OutputMode::Json => output.json().to_string(),
            };
            if output.exit_code() == SUCCESS_EXIT_CODE {
                ProcessOutput::success(body)
            } else {
                ProcessOutput {
                    stdout: String::new(),
                    stderr: format!("{body}\n"),
                    exit_code: output.exit_code(),
                }
            }
        }
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Injectable parser-to-runner pull boundary. Invalid IDs never construct a
/// runtime or consult authenticated context. Production uses this same dispatch.
pub fn run_pull_with<I, T, F>(args: I, operation: F) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
    F: FnOnce(destiny_app::ItemInstanceId) -> Result<crate::pull_output::PullOutput, CliError>,
{
    let cli = match parse_process(args) {
        Ok(cli) => cli,
        Err(output) => return output,
    };
    let mode = cli.output_mode();
    match cli.command {
        Command::Pull(arguments) => dispatch_pull(&arguments, mode, operation),
        _ => ProcessOutput::failure(
            mode,
            CliError::from_category(ErrorCategory::UnsupportedOperation),
        ),
    }
}
fn dispatch_pull<F>(
    arguments: &crate::command::PullArgs,
    mode: OutputMode,
    operation: F,
) -> ProcessOutput
where
    F: FnOnce(destiny_app::ItemInstanceId) -> Result<crate::pull_output::PullOutput, CliError>,
{
    let item = match arguments.instance_id() {
        Ok(item) => item,
        Err(error) => return ProcessOutput::failure(mode, error),
    };
    match operation(item) {
        Ok(output) => pull_process_output(mode, &output),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}
/// Only Verified goes to stdout with exit zero. All other structured evidence
/// remains intact on stderr, following existing mutation JSON semantics.
pub fn pull_process_output(
    mode: OutputMode,
    output: &crate::pull_output::PullOutput,
) -> ProcessOutput {
    let body = match mode {
        OutputMode::Human => output.render_text(),
        OutputMode::Json => output.json().to_string(),
    };
    if output.exit_code() == SUCCESS_EXIT_CODE {
        ProcessOutput::success(body)
    } else {
        ProcessOutput {
            stdout: String::new(),
            stderr: format!("{body}\n"),
            exit_code: output.exit_code(),
        }
    }
}

/// Render one mutation outcome; typed refusals and partial progress are failures.
/// Like existing CLI errors, their sole object is written to stderr.
pub fn equip_process_output(
    mode: OutputMode,
    output: &crate::equip_output::EquipOutput,
) -> ProcessOutput {
    let body = match mode {
        OutputMode::Human => output.render_text(),
        OutputMode::Json => output.json().to_string(),
    };
    let exit_code = output.exit_code();
    if exit_code == SUCCESS_EXIT_CODE {
        ProcessOutput::success(body)
    } else {
        ProcessOutput {
            stdout: String::new(),
            stderr: format!("{body}\n"),
            exit_code,
        }
    }
}

/// Move successes go to stdout; refusals and partial progress go to stderr.
pub fn move_process_output(
    mode: OutputMode,
    output: &crate::move_output::MoveOutput,
) -> ProcessOutput {
    let body = match mode {
        OutputMode::Human => output.render_text(),
        OutputMode::Json => output.json().to_string(),
    };
    let exit_code = output.exit_code();
    if exit_code == SUCCESS_EXIT_CODE {
        ProcessOutput::success(body)
    } else {
        ProcessOutput {
            stdout: String::new(),
            stderr: format!("{body}\n"),
            exit_code,
        }
    }
}

/// Lock successes go to stdout; refusals and unverified progress go to stderr.
pub fn lock_process_output(
    mode: OutputMode,
    output: &crate::lock_output::LockOutput,
) -> ProcessOutput {
    let body = match mode {
        OutputMode::Human => output.render_text(),
        OutputMode::Json => output.json().to_string(),
    };
    let exit_code = output.exit_code();
    if exit_code == SUCCESS_EXIT_CODE {
        ProcessOutput::success(body)
    } else {
        ProcessOutput {
            stdout: String::new(),
            stderr: format!("{body}\n"),
            exit_code,
        }
    }
}

/// Runs explicit acquisition; success is rendered only after publication.
pub fn run_profile_sync<S: crate::runtime::ProfileSyncOperations + ?Sized>(
    service: &mut S,
    mode: OutputMode,
    freshness: ReadFreshness,
) -> ProcessOutput {
    match service.run_profile_sync(freshness.policy, freshness.now) {
        Ok(report) => ProcessOutput::success(crate::output::render_profile_sync(mode, &report)),
        Err(error) => ProcessOutput::failure(mode, CliError::from(error)),
    }
}

/// Runs a public Manifest sync through its credential-free composition.
pub fn run_manifest<M: crate::runtime::ManifestFactory>(
    factory: &mut M,
    mode: OutputMode,
) -> ProcessOutput {
    match factory.sync_with_clarity() {
        Ok(report) => ProcessOutput::success(crate::output::render_manifest_with_clarity(
            mode,
            &report.manifest,
            &report.clarity,
        )),
        Err(error) => ProcessOutput::failure(mode, error),
    }
}

/// Runs the authenticated weekly reset with an optional evaluation instant.
pub fn run_weekly_reset<W: crate::runtime::WeeklyResetFactory>(
    factory: &mut W,
    mode: OutputMode,
    requested: Option<Timestamp>,
) -> ProcessOutput {
    match factory.weekly_reset(requested) {
        Ok(reset) => ProcessOutput::success(crate::output::render_weekly_reset(
            mode,
            &reset,
            &chrono::Local,
        )),
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
pub(crate) fn login_result(mode: OutputMode, outcome: AuthorizationOutcome) -> ProcessOutput {
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
        _ => ProcessOutput::failure(mode, clap_argument_failure(error)),
    }
}

fn clap_argument_failure(error: &clap::Error) -> CliError {
    use clap::error::{ContextKind, ErrorKind};
    // Only emit known schema names, never raw arguments, values, or clap's
    // rendered diagnostic (which can contain an authorization callback URI).
    let context = error.get(ContextKind::InvalidArg).map(ToString::to_string);
    let argument = context.as_deref().and_then(|context| {
        [
            "--limit",
            "--slot",
            "--element",
            "--class",
            "--stat",
            "--perk",
            "--mod",
            "--archetype",
            "--text",
            "--type",
            "--tag",
            "--character",
            "--item-hash",
            "--perk-hash",
            "--min-power",
            "--json",
            "--mode",
            "--callback",
            "--details",
            "--debug",
            "--include-available",
        ]
        .into_iter()
        .find(|flag| context.split_whitespace().next() == Some(*flag))
    });
    if let Some(argument) = argument {
        let reason = match error.kind() {
            ErrorKind::ArgumentConflict => {
                "cannot be used more than once or with conflicting arguments"
            }
            ErrorKind::ValueValidation | ErrorKind::InvalidValue => match argument {
                "--slot" => {
                    "expected one of kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown"
                }
                "--element" => "expected one of kinetic, arc, solar, void, stasis, strand",
                "--class" => "expected one of titan, hunter, warlock, any, unknown",
                "--limit" | "--perk-hash" | "--min-power" => {
                    "expected a non-negative integer in the supported range"
                }
                "--item-hash" => "expected a signed 64-bit integer",
                "--mode" => "expected one of auto, browser, headless",
                "--callback" => "expected one of paste, handler",
                _ => "expected a valid value",
            },
            ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => "a value is required",
            _ => "argument is not valid here",
        };
        return CliError::invalid_argument(argument, reason);
    }
    let reason = match error.kind() {
        ErrorKind::UnknownArgument => "unrecognized argument; use --help for accepted arguments",
        ErrorKind::InvalidSubcommand => "unrecognized subcommand; use --help for accepted commands",
        ErrorKind::MissingRequiredArgument => {
            "required argument is missing; use --help for required arguments"
        }
        _ => "invalid argument syntax; use --help for accepted arguments",
    };
    CliError::invalid_argument("arguments", reason)
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
    // Test-only command log mirrors the shared bounded request without boxing.
    #[allow(clippy::large_enum_variant)]
    enum Recorded {
        List,
        Sync,
        Search(SearchInventoryRequest),
        Inspect(InspectItemRequest),
        Postmaster(Option<CharacterId>),
        Subclass(Option<CharacterId>),
        SubclassOptions(Option<CharacterId>, Option<destiny_app::SubclassElement>),
        Artifact(Option<CharacterId>),
        Loadouts(Option<CharacterId>),
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
        sync_failure: Option<ErrorCategory>,
        postmaster: Result<destiny_app::PostmasterResult, AppError>,
        list: Result<CharacterListResult, AppError>,
        search: Result<InventorySearchResult, AppError>,
        inspect: Result<Resolution<ItemInspection>, AppError>,
    }

    impl RecordingService {
        fn new() -> Self {
            Self {
                calls: Rc::new(RefCell::new(Vec::new())),
                sync_failure: None,
                postmaster: Ok(destiny_app::PostmasterResult {
                    metadata: metadata(FreshnessStatus::Fresh),
                    profile_currencies_coverage: Default::default(),
                    characters: vec![],
                }),
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

    impl crate::runtime::ProfileSyncOperations for RecordingService {
        fn run_profile_sync(
            &mut self,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<crate::runtime::ProfileSyncReport, AppError> {
            self.record(Recorded::Sync, policy, now);
            if let Some(category) = self.sync_failure {
                return Err(AppError::new(category));
            }
            Ok(crate::runtime::ProfileSyncReport {
                metadata: metadata(FreshnessStatus::Fresh).sync,
                previous_revision: Some(SyncRevision::new(3)),
                discarded: false,
                profile_minted_at: None,
            })
        }
    }

    #[test]
    fn postmaster_dispatch_forwards_selector_policy_time_and_sanitized_errors() {
        for selector in [None, Some("123"), Some("HuNtEr")] {
            let mut service = RecordingService::new();
            let mut args = vec!["ghostlink", "postmaster", "--json"];
            if let Some(selector) = selector {
                args.extend(["--character", selector]);
            }
            let output = run(
                &mut service,
                args,
                ReadFreshness {
                    policy: TTL,
                    now: NOW,
                },
            );
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            assert_eq!(
                service.recorded(),
                vec![(
                    Recorded::Postmaster(selector.map(CharacterId::new)),
                    TTL,
                    NOW
                )]
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap()["kind"],
                "postmaster"
            );
        }
        for category in [
            ErrorCategory::AuthorizationRequired,
            ErrorCategory::ProviderUnavailable,
            ErrorCategory::LocalStorage,
        ] {
            let mut service = RecordingService::new();
            service.postmaster = Err(AppError::new(category));
            let output = run(
                &mut service,
                ["ghostlink", "postmaster", "--json"],
                ReadFreshness {
                    policy: TTL,
                    now: NOW,
                },
            );
            assert_eq!(
                output.exit_code,
                CliError::from_category(category).exit_code()
            );
            assert!(output.stdout.is_empty());
            assert_eq!(service.recorded().len(), 1);
        }
    }

    impl QueryOperations for RecordingService {
        fn postmaster(
            &mut self,
            character_id: Option<&CharacterId>,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<destiny_app::PostmasterResult, AppError> {
            self.record(Recorded::Postmaster(character_id.cloned()), policy, now);
            self.postmaster.clone()
        }

        fn subclass_options(
            &mut self,
            character_id: Option<&destiny_app::CharacterId>,
            element: Option<destiny_app::SubclassElement>,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<destiny_app::SubclassOptionsResult, AppError> {
            self.record(
                Recorded::SubclassOptions(character_id.cloned(), element),
                policy,
                now,
            );
            Ok(destiny_app::SubclassOptionsResult {
                reference_character_id: None,
                characters: vec![],
            })
        }

        fn character_subclasses(
            &mut self,
            character_id: Option<&CharacterId>,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<destiny_app::SubclassResult, AppError> {
            self.record(Recorded::Subclass(character_id.cloned()), policy, now);
            Ok(destiny_app::SubclassResult {
                characters: Vec::new(),
            })
        }

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
            let mut result = self.search.clone()?;
            if let Some(character_id) = request
                .instances
                .as_ref()
                .and_then(|query| query.character_id.as_ref())
            {
                result.hits.retain(|hit| {
                    hit.instance.location == ItemLocation::Character(character_id.clone())
                });
            }
            Ok(result)
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

    impl destiny_app::ArtifactOperations for RecordingService {
        fn artifact(
            &mut self,
            selector: Option<&CharacterId>,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<destiny_app::ArtifactResult, AppError> {
            self.record(Recorded::Artifact(selector.cloned()), policy, now);
            Ok(destiny_app::ArtifactResult {
                points: None,
                characters: vec![],
            })
        }
    }

    impl destiny_app::LoadoutOperations for RecordingService {
        fn loadouts(
            &mut self,
            selector: Option<&CharacterId>,
            policy: FreshnessPolicy,
            now: Timestamp,
        ) -> Result<destiny_app::LoadoutsResult, AppError> {
            self.record(Recorded::Loadouts(selector.cloned()), policy, now);
            Ok(destiny_app::LoadoutsResult { characters: vec![] })
        }
    }

    #[test]
    fn loadouts_dispatches_selector_and_freshness_once() {
        for selector in [None, Some("hunter"), Some("123")] {
            for detail in [false, true] {
                let mut service = RecordingService::new();
                let mut args = vec!["ghostlink", "--json", "loadouts"];
                if let Some(selector) = selector {
                    args.extend(["--character", selector]);
                }
                if detail {
                    args.push("--detail");
                }
                let cli = parse_process(args).unwrap();
                let mode = cli.output_mode();
                let ReadRequest::Loadouts {
                    character_id,
                    detail,
                    ..
                } = cli.into_read_request().unwrap()
                else {
                    panic!("expected loadouts")
                };
                let output = super::dispatch_loadouts(
                    &mut service,
                    character_id.as_ref(),
                    detail,
                    mode,
                    freshness(),
                );
                assert_eq!(output.exit_code, 0);
                assert!(output.stderr.is_empty());
                assert_eq!(
                    service.recorded(),
                    vec![(Recorded::Loadouts(selector.map(CharacterId::new)), TTL, NOW)]
                );
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
                    serde_json::json!({"kind":"loadouts", "characters":[]})
                );
            }
        }
    }

    #[test]
    fn loadouts_dispatch_maps_service_errors() {
        struct Failed;
        impl destiny_app::LoadoutOperations for Failed {
            fn loadouts(
                &mut self,
                _: Option<&CharacterId>,
                _: FreshnessPolicy,
                _: Timestamp,
            ) -> Result<destiny_app::LoadoutsResult, AppError> {
                Err(AppError::new(ErrorCategory::DefinitionsUnavailable))
            }
        }
        let output =
            super::dispatch_loadouts(&mut Failed, None, false, OutputMode::Json, freshness());
        assert_ne!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("definitions_unavailable"));
    }

    #[test]
    fn artifact_dispatches_selector_and_freshness_once() {
        for selector in [None, Some("hunter"), Some("123")] {
            let mut service = RecordingService::new();
            let mut args = vec!["ghostlink", "--json", "artifact"];
            if let Some(selector) = selector {
                args.extend(["--character", selector]);
            }
            let cli = parse_process(args).unwrap();
            let mode = cli.output_mode();
            let ReadRequest::Artifact { character_id } = cli.into_read_request().unwrap() else {
                panic!("expected artifact")
            };
            let output =
                super::dispatch_artifact(&mut service, character_id.as_ref(), mode, freshness());
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            assert_eq!(
                service.recorded(),
                vec![(Recorded::Artifact(selector.map(CharacterId::new)), TTL, NOW)]
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
                serde_json::json!({"kind":"artifact", "points":null, "characters":[]})
            );
        }
    }

    #[test]
    fn subclass_options_dispatches_filters_and_freshness_once() {
        for selector in [None, Some("hunter"), Some("123")] {
            for element in [None, Some(destiny_app::SubclassElement::Prismatic)] {
                for locked in [false, true] {
                    let mut service = RecordingService::new();
                    let mut args = vec!["ghostlink", "subclass-options"];
                    if let Some(selector) = selector {
                        args.extend(["--character", selector]);
                    }
                    if let Some(element) = element {
                        args.extend(["--element", element.as_str()]);
                    }
                    if locked {
                        args.push("--locked");
                    } else {
                        args.push("--json");
                    }
                    let output = run(&mut service, args, freshness());
                    assert_eq!(output.exit_code, 0);
                    assert!(output.stderr.is_empty());
                    assert_eq!(
                        service.recorded(),
                        vec![(
                            Recorded::SubclassOptions(selector.map(CharacterId::new), element),
                            TTL,
                            NOW
                        )]
                    );
                    if !locked {
                        assert_eq!(
                            serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
                            serde_json::json!({"kind":"subclass_options", "reference_character_id":null, "characters":[]})
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn subclass_options_invalid_requests_never_dispatch() {
        for args in [
            vec!["ghostlink", "subclass-options", "--character", " "],
            vec!["ghostlink", "subclass-options", "--element", "kinetic"],
            vec!["ghostlink", "subclass-options", "--locked", "--json"],
            vec!["ghostlink", "--json", "subclass-options", "--locked"],
        ] {
            let mut service = RecordingService::new();
            let output = run(&mut service, args, freshness());
            assert_eq!(output.exit_code, 2);
            assert!(output.stdout.is_empty());
            assert!(service.recorded().is_empty());
        }
    }

    #[test]
    fn subclass_dispatches_selector_and_freshness_once() {
        for selector in [None, Some("hunter"), Some("123")] {
            let mut service = RecordingService::new();
            let mut args = vec!["ghostlink", "--json", "subclass"];
            if let Some(selector) = selector {
                args.extend(["--character", selector]);
            }
            let output = run(&mut service, args, freshness());
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            assert_eq!(
                service.recorded(),
                vec![(Recorded::Subclass(selector.map(CharacterId::new)), TTL, NOW)]
            );
            let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(value["kind"], "subclass");
            assert_eq!(value["characters"], serde_json::json!([]));
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
            match self.failure.clone() {
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
            armor: Default::default(),
            non_transferrable: None,
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
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
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: Vec::new(),
            instance_id: ItemInstanceId::new(identifier),
            item_hash: ItemHash::new(hash),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: Some(1_350),
            stats: Vec::new(),
            plugs: Vec::new(),
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
                date_last_played: None,
            }],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn search_result() -> InventorySearchResult {
        InventorySearchResult {
            next_offset: None,
            resolved: Default::default(),
            candidates: vec![definition(1, "Gjallarhorn")],
            hits: vec![InventorySearchHit {
                definition: definition(1, "Gjallarhorn"),
                instance: instance("inst-1", 1),
                perk_names: Vec::new(),
                mod_names: Vec::new(),
                available_perks: Vec::new(),
                available_mods: Vec::new(),
                archetype: None,
                stats: Vec::new(),
            }],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn inspection() -> ItemInspection {
        ItemInspection {
            plug_details: Vec::new(),
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            archetype: None,
            available_perks: Vec::new(),
            available_mods: Vec::new(),
            stats: Vec::new(),
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
            match (self.failure.clone(), self.script.clone()) {
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
    fn automatic_manifest_sync_once_preserves_json_and_login_success() {
        struct Manifest {
            missing: bool,
            calls: usize,
            fail: bool,
        }
        impl crate::runtime::ManifestFactory for Manifest {
            fn definitions_missing(&mut self) -> Result<bool, CliError> {
                Ok(self.missing)
            }
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                self.calls += 1;
                if self.fail {
                    return Err(CliError::from_category(ErrorCategory::ProviderUnavailable));
                }
                self.missing = false;
                Ok(destiny_app::ManifestSyncReport::Unchanged {
                    version: DefinitionVersion::new("real-v1"),
                    record_count: 1,
                    world_counts: [1; 5],
                    stat_count: 1,
                })
            }
        }
        for command in ["sync", "list-characters", "login"] {
            for missing in [false, true] {
                for fail in [false, true] {
                    let mut manifest = Manifest {
                        missing,
                        calls: 0,
                        fail,
                    };
                    let mut read = FakeFactory::new(RecordingService::new());
                    let mut login = FakeLoginFactory::default();
                    let output = super::run_binary_with_manifest(
                        &mut read,
                        &mut login,
                        &mut manifest,
                        &mut |_| Ok(()),
                        argv(["--json", command]),
                    );
                    assert_eq!(manifest.calls, usize::from(missing));
                    let failure = missing && fail && command != "login";
                    assert_eq!(output.exit_code == 0, !failure);
                    if !failure {
                        assert!(serde_json::from_str::<serde_json::Value>(&output.stdout).is_ok());
                        assert!(!output.stdout.contains("syncing manifest"));
                    } else {
                        assert!(output.stdout.is_empty());
                    }
                    if command == "login" && missing && fail {
                        assert!(output.stderr.contains("run ghostlink sync-manifest"));
                    }
                }
            }
        }
    }

    #[test]
    fn sync_routing_builds_once_and_dispatches_only_sync() {
        struct Public;
        impl crate::runtime::ManifestFactory for Public {
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                panic!("no public sync")
            }
        }
        impl crate::runtime::WeeklyResetFactory for Public {
            fn weekly_reset(
                &mut self,
                _: Option<Timestamp>,
            ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
                panic!("no public read")
            }
        }
        for failure in [
            None,
            Some(ErrorCategory::ProviderRateLimit),
            Some(ErrorCategory::ProviderUnavailable),
            Some(ErrorCategory::LocalStorage),
        ] {
            let mut service = RecordingService::new();
            service.sync_failure = failure;
            let mut read = FakeFactory::new(service.clone());
            read.freshness = ReadFreshness {
                policy: FreshnessPolicy { ttl_seconds: 777 },
                now: Timestamp::new(123),
            };
            let mut login = FakeLoginFactory::default();
            let output = super::run_binary_with_public(
                &mut read,
                &mut login,
                &mut Public,
                &mut Public,
                &mut |_| panic!("no login instruction"),
                argv(["sync", "--json"]),
            );
            assert_eq!(read.build_count(), 1);
            assert_eq!(
                service.recorded(),
                vec![(Recorded::Sync, read.freshness.policy, read.freshness.now)]
            );
            assert!(login.modes().is_empty());
            match failure {
                None => {
                    assert_eq!(output.exit_code, 0);
                    assert!(output.stderr.is_empty());
                    assert!(output.stdout.contains("\"publication\":\"committed\""));
                }
                Some(category) => {
                    assert_eq!(
                        output.exit_code,
                        CliError::from_category(category).exit_code()
                    );
                    assert!(output.stdout.is_empty());
                    assert!(
                        output
                            .stderr
                            .contains(CliError::from_category(category).code())
                    );
                }
            }
        }
        let mut read = FakeFactory::new(RecordingService::new());
        read.failure = Some(CliError::from_category(
            ErrorCategory::AuthorizationRequired,
        ));
        let (output, _) = run_read_boundary(&mut read, ["--json", "sync"]);
        assert_eq!(output.exit_code, 10);
        assert_eq!(read.build_count(), 1);
        assert!(read.service.recorded().is_empty());
        let (output, _) = run_read_boundary(&mut read, ["sync", "extra"]);
        assert_eq!(output.exit_code, 2);
        assert_eq!(read.build_count(), 1);
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
        struct Weekly(usize, Vec<Option<Timestamp>>);
        impl crate::runtime::WeeklyResetFactory for Weekly {
            fn weekly_reset(
                &mut self,
                requested: Option<Timestamp>,
            ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
                self.0 += 1;
                self.1.push(requested);
                Err(CliError::from_category(
                    ErrorCategory::DefinitionsUnavailable,
                ))
            }
        }
        let mut read = FakeFactory::new(RecordingService::new());
        let mut login = FakeLoginFactory::default();
        let mut weekly = Weekly(0, Vec::new());
        for arguments in [
            vec!["weekly-reset", "--unknown"],
            vec!["weekly-reset", "--help"],
            vec!["weekly-reset", "--date", "2026-10-6"],
            vec!["weekly-reset", "--date", "2026-13-01", "--json"],
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
                format!(
                    "{{\"error\":{{\"code\":\"invalid_request\",\"message\":\"invalid {flag}: cannot be used with --json\"}}}}\n"
                )
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
        let output = super::run_binary_with_public(
            &mut read,
            &mut login,
            &mut Public,
            &mut weekly,
            &mut |_| panic!("weekly read must not present login"),
            argv(["weekly-reset", "--date", "2026-10-06", "--json"]),
        );
        assert_eq!(output.exit_code, 33);
        assert_eq!(weekly.0, 2);
        assert_eq!(
            weekly.1,
            vec![
                None,
                Some(
                    destiny_adapters::calendar::instant_for_date("2026-10-06", &chrono::Local)
                        .unwrap()
                )
            ]
        );
        assert_eq!(read.build_count(), 0);
        assert!(login.modes().is_empty());
    }

    #[test]
    fn search_invalid_arguments_have_specific_text_and_json_messages() {
        for (arguments, expected) in [
            (
                vec!["--location", "orbit"],
                "invalid --location: expected one of vault, character",
            ),
            (
                vec!["--equipped", "yes"],
                "invalid --equipped: expected true or false",
            ),
            (
                vec!["--limit", "0"],
                "invalid --limit: must be between 1 and 1000",
            ),
            (
                vec!["--limit", "1001"],
                "invalid --limit: must be between 1 and 1000",
            ),
            (
                vec!["--slot", "banana"],
                "invalid --slot: expected one of kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown",
            ),
            (
                vec!["--stat", "Range>=abc"],
                "invalid --stat: expected Name>=N, Name<=N, Name>N, Name<N or Name=N (N must be a signed 32-bit integer)",
            ),
            (
                vec![
                    "--perk", "first", "--perk", "first", "--perk", "first", "--perk", "first",
                    "--perk", "first", "--perk", "first", "--perk", "first", "--perk", "first",
                    "--perk", "first",
                ],
                "invalid --perk: at most 8 perk names",
            ),
        ] {
            for json in [false, true] {
                let mut args = vec!["search-inventory"];
                args.extend(arguments.iter().copied());
                if json {
                    args.push("--json");
                }
                let mut service = RecordingService::new();
                let output = run(&mut service, argv(args), freshness());
                assert_eq!(output.exit_code, 2);
                if json {
                    let value: serde_json::Value = serde_json::from_str(&output.stderr).unwrap();
                    assert_eq!(value["error"]["code"], "invalid_request");
                    assert_eq!(value["error"]["message"], expected);
                } else {
                    assert_eq!(output.stderr, format!("error: {expected}\n"));
                }
                assert!(service.recorded().is_empty());
            }
        }
    }

    #[test]
    fn clap_details_never_echo_auth_values_or_control_bearing_search_values() {
        for arguments in [
            vec!["login", "--mode", "secret\n\u{7}sentinel"],
            vec!["login", "--callback", "secret-sentinel"],
            vec!["callback", "secret-sentinel", "extra-secret-sentinel"],
            vec!["search-inventory", "--slot", "secret\n\u{7}sentinel"],
            vec![
                "search-inventory",
                "--archetype",
                "secret-sentinel",
                "--archetype",
                "other-secret-sentinel",
            ],
        ] {
            for json in [false, true] {
                let mut args = arguments.clone();
                if json {
                    args.push("--json");
                }
                let output = super::parse_process(argv(args)).unwrap_err();
                assert_eq!(output.exit_code, 2);
                assert!(!output.stderr.contains("sentinel"));
                assert!(!output.stderr.contains('\u{7}'));
                assert_eq!(output.stderr.lines().count(), 1);
                assert!(output.stderr.chars().count() <= 600);
            }
        }
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
        assert_eq!(
            output.stderr,
            "error: invalid --limit: expected a non-negative integer in the supported range\n"
        );
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
            "{\"error\":{\"code\":\"invalid_request\",\"message\":\"invalid arguments: unrecognized argument; use --help for accepted arguments\"}}\n"
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
        assert_eq!(
            output.stderr,
            "error: invalid --json: argument is not valid here\n"
        );
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
        assert_eq!(
            output.stderr,
            "error: invalid --limit: must be between 1 and 1000\n"
        );
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
    fn dim_query_dispatch_and_diagnostics() {
        for query in ["is:weapon -is:locked", ""] {
            let mut service = RecordingService::new();
            let output = run(
                &mut service,
                argv(&["search-inventory", query, "--tier", "Exotic"]),
                freshness(),
            );
            assert_eq!(output.exit_code, 0, "{}", output.stderr);
            let recorded = service.recorded();
            let Recorded::Search(request) = &recorded[0].0 else {
                panic!("expected search")
            };
            assert_eq!(
                request.query,
                Some(destiny_app::search_query::compile(query).unwrap())
            );
            assert_eq!(
                request.definitions.tier,
                Some(destiny_app::SearchTerm::new("Exotic").unwrap())
            );
        }
        let mut service = RecordingService::new();
        let output = run(
            &mut service,
            argv(&["search-inventory", "is:weapon mystery:x"]),
            freshness(),
        );
        assert_eq!(output.exit_code, 2);
        assert!(
            output.stderr.contains("unknown_filter"),
            "{}",
            output.stderr
        );
        assert!(output.stderr.contains("10..19"));
        assert!(output.stderr.contains('^'));
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn dim_filters_help_is_registry_generated_and_stable() {
        let mut service = RecordingService::new();
        let first = run(
            &mut service,
            argv(&["search-inventory", "--filters"]),
            freshness(),
        );
        let second = run(
            &mut service,
            argv(&["search-inventory", "--filters"]),
            freshness(),
        );
        assert_eq!(first.exit_code, 0);
        assert_eq!(first.stdout, second.stdout);
        for filter in destiny_app::search_query::list_filters() {
            for key in filter.keys {
                assert!(first.stdout.contains(key), "missing {key}");
            }
            assert!(first.stdout.contains(filter.description));
            for example in filter.examples {
                assert!(first.stdout.contains(example));
            }
        }
        assert!(first.stdout.contains("precedence"));
        assert!(service.recorded().is_empty());
    }

    #[test]
    fn search_dispatches_the_exact_validated_request_once() {
        let expected = SearchInventoryRequest {
            query: None,
            definitions: destiny_app::DefinitionSearchQuery {
                tier: None,
                text: Some(destiny_app::SearchTerm::new("gjallar").unwrap()),
                item_type: None,
                tags: Vec::new(),
                element: None,
                slot: None,
                class_type: None,
                limit: 5,
            },
            instances: Some(destiny_app::InstanceSearchQuery {
                offset: 0,
                sort: None,
                location: None,
                equipped: None,
                include_available: false,
                item_hashes: destiny_app::ItemHashFilter::AnyOf(vec![ItemHash::new(1)]),
                perk_hash: None,
                perk_name: None,
                mod_name: None,
                archetype: None,
                stats: Vec::new(),
                character_id: None,
                definitions: Some(Box::new(destiny_app::DefinitionSearchQuery {
                    tier: None,
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

    fn hunter_search_service() -> RecordingService {
        let mut service = RecordingService::new();
        let mut characters = character_list();
        let mut hunter = characters.characters[0].clone();
        hunter.id = CharacterId::new("123");
        hunter.class = Some("Hunter".into());
        characters.characters.push(hunter);
        service.list = Ok(characters);
        let mut search = search_result();
        search.hits[0].instance.location = ItemLocation::Character(CharacterId::new("123"));
        service.search = Ok(search);
        service
    }

    #[test]
    fn search_character_selector_resolves_class_and_exact_id_before_search() {
        for selector in ["hunter", "HuNtEr", "123"] {
            let mut service = hunter_search_service();
            let output = run(
                &mut service,
                argv(&["--json", "search-inventory", "--character", selector]),
                freshness(),
            );
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(json["hits"].as_array().unwrap().len(), 1, "{selector}");
            let calls = service.recorded();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0], (Recorded::List, TTL, NOW));
            let (Recorded::Search(request), policy, now) = &calls[1] else {
                panic!("expected search after character selection");
            };
            assert_eq!((*policy, *now), (TTL, NOW));
            assert_eq!(
                request.instances.as_ref().unwrap().character_id,
                Some(CharacterId::new("123"))
            );
        }
    }

    #[test]
    fn search_character_exact_id_wins_over_class_and_valid_empty_search_succeeds() {
        for empty in [false, true] {
            let mut service = hunter_search_service();
            // A class-looking exact ID must win over the other Hunter.
            service.list.as_mut().unwrap().characters[0].id = CharacterId::new("hunter");
            let hits = &mut service.search.as_mut().unwrap().hits;
            hits[0].instance.location = ItemLocation::Character(CharacterId::new("hunter"));
            if empty {
                hits.clear();
            }
            let output = run(
                &mut service,
                argv(&["--json", "search-inventory", "--character", "hunter"]),
                freshness(),
            );
            assert_eq!(output.exit_code, 0);
            assert!(output.stderr.is_empty());
            let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(json["hits"].as_array().unwrap().len(), usize::from(!empty));
            let calls = service.recorded();
            assert_eq!(calls[0], (Recorded::List, TTL, NOW));
            assert_eq!(calls.len(), 2);
            let Recorded::Search(request) = &calls[1].0 else {
                panic!("expected search after exact ID selection");
            };
            assert_eq!(
                request.instances.as_ref().unwrap().character_id,
                Some(CharacterId::new("hunter"))
            );
        }
    }

    #[test]
    fn search_character_selector_rejects_unknown_before_search() {
        for selector in ["nonsense", "999"] {
            assert_search_selector_rejected(selector, false, CliError::invalid_request());
        }
    }

    #[test]
    fn search_character_selector_rejects_ambiguous_before_search() {
        assert_search_selector_rejected(
            "hunter",
            true,
            CliError::from_category(ErrorCategory::AmbiguousMatch),
        );
    }

    fn assert_search_selector_rejected(selector: &str, duplicate: bool, expected: CliError) {
        let mut service = hunter_search_service();
        if duplicate {
            let characters = &mut service.list.as_mut().unwrap().characters;
            let mut hunter = characters[1].clone();
            hunter.id = CharacterId::new("456");
            characters.push(hunter);
        }
        let output = run(
            &mut service,
            argv(&["--json", "search-inventory", "--character", selector]),
            freshness(),
        );
        assert_eq!(output.exit_code, expected.exit_code(), "{selector}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            format!("{}\n", super::render_error(OutputMode::Json, &expected))
        );
        assert_eq!(service.recorded(), [(Recorded::List, TTL, NOW)]);
    }

    #[test]
    fn search_character_selector_propagates_character_read_failure() {
        let mut service = hunter_search_service();
        service.list = Err(AppError::new(ErrorCategory::ProviderUnavailable));
        let output = run(
            &mut service,
            argv(&["--json", "search-inventory", "--character", "hunter"]),
            freshness(),
        );
        assert_eq!(
            output.exit_code,
            CliError::from_category(ErrorCategory::ProviderUnavailable).exit_code()
        );
        assert!(output.stdout.is_empty());
        assert_eq!(service.recorded(), [(Recorded::List, TTL, NOW)]);
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
    fn search_detail_flag_dispatches_once_and_preserves_legacy_output() {
        for mode in [OutputMode::Json, OutputMode::Human] {
            for detail in [false, true] {
                let mut service = RecordingService::new();
                let mut args = vec!["search-inventory"];
                if mode == OutputMode::Json {
                    args.push("--json");
                }
                if detail {
                    args.push("--detail");
                }
                let output = run(&mut service, argv(&args), freshness());
                assert_eq!(output.exit_code, 0);
                assert!(output.stderr.is_empty());
                let expected = if detail {
                    crate::output::render_search_detail(mode, &search_result())
                } else {
                    crate::output::render_search(mode, &search_result())
                };
                assert_eq!(output.stdout.trim_end(), expected);
                assert_eq!(service.recorded().len(), 1);
                if mode == OutputMode::Json {
                    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
                    assert_eq!(value["hits"][0].get("instance").is_some(), detail);
                    assert_eq!(value["hits"][0].get("instance_id").is_some(), !detail);
                }
            }
        }
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
        assert_eq!(
            output.stderr,
            "error: invalid arguments: unrecognized argument; use --help for accepted arguments\n"
        );

        // A parsed but semantically invalid request.
        let (output, _) = run_read_boundary(&mut factory, &["search-inventory", "--limit", "0"]);
        assert_eq!(output.exit_code, 2);
        assert_eq!(
            output.stderr,
            "error: invalid --limit: must be between 1 and 1000\n"
        );

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
        // environment variable, credential file, database, clock, or transport.
        let output: ProcessOutput = run_binary(argv(&["inspect-item", "--unknown"]));
        assert_eq!(output.exit_code, 2);
        assert_eq!(
            output.stderr,
            "error: invalid arguments: unrecognized argument; use --help for accepted arguments\n"
        );
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
            (None, CallbackMode::Auto),
            (Some("auto"), CallbackMode::Auto),
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
        assert_eq!(
            output.stderr,
            "error: invalid --mode: expected one of auto, browser, headless\n"
        );
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
