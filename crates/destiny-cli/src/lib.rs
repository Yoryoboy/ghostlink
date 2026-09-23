//! Library surface of the read-only Destiny CLI.
//!
//! The crate is both this library and the `destiny-cli` binary. The library
//! owns the three read commands, the validated shared request each one derives,
//! the deterministic human and JSON renderers, and the stable typed exits. The
//! binary is a thin process boundary over this surface.
//!
//! Nothing here dispatches a read: command parsing, request validation, and
//! rendering are pure, and the shared freshness-aware services are invoked by
//! the runner boundary in later work.

pub mod command;
pub mod output;

pub use command::{
    Cli, CliError, Command, ErrorOutcome, InspectItemArgs, ReadRequest, SUCCESS_CODE,
    SUCCESS_EXIT_CODE, SearchInventoryArgs, category_outcome, inspection_outcome,
};
pub use output::{OutputMode, render_characters, render_error, render_inspection, render_search};
