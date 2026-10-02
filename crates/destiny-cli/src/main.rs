//! The `ghostlink` process boundary.
//!
//! All argument parsing, request validation, runtime and login composition, and
//! output choice live in the library. The binary only forwards the process
//! arguments and emits the fully rendered result.

use std::process::ExitCode;

fn main() -> ExitCode {
    destiny_cli::run_binary(std::env::args_os()).emit()
}
