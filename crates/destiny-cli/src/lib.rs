//! Library surface of the read-only Destiny CLI.
//!
//! The crate is both this library and the `ghostlink` binary. The library owns the
//! three read commands, the interactive login command, the validated shared
//! request each one derives, the deterministic human and JSON renderers, the
//! injectable production read-runtime and login composition roots, and the stable
//! typed exits. The binary is a thin process boundary over this surface.
//!
//! Nothing here dispatches a read or starts an authorization directly: command
//! parsing, request validation, and rendering are pure, the shared
//! freshness-aware services are invoked only through the [`runner`] boundary,
//! whose production runtime is composed in [`runtime`], and the authorization
//! service is composed only in [`login`].

pub mod command;
pub mod login;
pub mod output;
pub mod runner;
pub mod runtime;

pub use command::{
    CallbackArgs, CallbackMode, Cli, CliError, Command, ErrorOutcome, InspectItemArgs, LoginArgs,
    LoginMode, ReadRequest, SUCCESS_CODE, SUCCESS_EXIT_CODE, SearchInventoryArgs, category_outcome,
    inspection_outcome,
};
pub use login::{
    LoginFactory, LoginInstruction, LoginInstructionSource, LoginOperations, LoginSession,
    NoInstruction, ProductionLoginFactory,
};
pub use output::{
    OutputMode, render_characters, render_error, render_inspection, render_login_authorized,
    render_login_instruction, render_manifest, render_search,
};
pub use runner::{
    ProcessOutput, ReadFreshness, dispatch, parse_process, run, run_binary, run_binary_with,
    run_binary_with_manifest, run_cli, run_login, run_manifest,
};
pub use runtime::{
    ComposedReadService, ComposingManifestFactory, ComposingRuntimeFactory, ManifestFactory,
    ManifestFoundations, ProductionFoundations, ReadFoundations, ReadRuntime, RuntimeFactory,
    compose, production_runtime_factory,
};
