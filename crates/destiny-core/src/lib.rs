//! Provider- and transport-free domain contracts.

pub mod error;

pub use error::{AppError, ErrorCategory};

/// Marker function for the initial provider-free foundation.
pub fn foundation_ready() -> bool {
    true
}
