//! Application services that depend inward on [`destiny_core`].

/// Reports that the application foundation is available.
pub fn foundation_ready() -> bool {
    destiny_core::foundation_ready()
}
