//! Library surface of the read-only Destiny CLI.
//!
//! The crate is both this library and the `destiny-cli` binary. The library
//! owns the three read commands, the validated shared request each one derives,
//! the deterministic human and JSON renderers, the injectable production
//! read-runtime composition root, and the stable typed exits. The binary is a
//! thin process boundary over this surface.
//!
//! Nothing here dispatches a read directly: command parsing, request
//! validation, and rendering are pure, and the shared freshness-aware services
//! are invoked only through the [`runner`] boundary, whose production runtime is
//! composed in [`runtime`]. The binary is a thin process boundary over this
//! surface.

pub mod command;
pub mod output;
pub mod runner;
pub mod runtime;

pub use command::{
    Cli, CliError, Command, ErrorOutcome, InspectItemArgs, ReadRequest, SUCCESS_CODE,
    SUCCESS_EXIT_CODE, SearchInventoryArgs, category_outcome, inspection_outcome,
};
pub use output::{OutputMode, render_characters, render_error, render_inspection, render_search};
pub use runner::{
    ProcessOutput, ReadFreshness, dispatch, parse_process, run, run_binary, run_binary_with,
    run_cli,
};
pub use runtime::{
    ComposedReadService, ComposingRuntimeFactory, ProductionFoundations, ReadFoundations,
    ReadRuntime, RuntimeFactory, compose, production_runtime_factory,
};
