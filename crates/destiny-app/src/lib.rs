//! Application-facing configuration, services, and sanitized diagnostics.

use std::fmt;

mod config;

pub use config::{CallbackUri, Config};
pub use destiny_core::{AppError, ErrorCategory};

/// A typed, adapter-neutral startup diagnostic.
///
/// Adapters can present this value without receiving configuration details or
/// constructing their own error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupDiagnostic(AppError);

impl StartupDiagnostic {
    pub const fn from_error(error: AppError) -> Self {
        Self(error)
    }
}

impl fmt::Display for StartupDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "error: {}", self.0)
    }
}

/// Converts an application error into the only startup diagnostic adapters
/// should present.
pub const fn startup_diagnostic(error: AppError) -> StartupDiagnostic {
    StartupDiagnostic::from_error(error)
}

/// Reports that the application foundation is available.
pub fn foundation_ready() -> bool {
    destiny_core::foundation_ready()
}

/// Loads and validates required runtime configuration.
pub fn load_config() -> Result<Config, AppError> {
    Config::from_env()
}

#[cfg(test)]
mod tests {
    use super::{AppError, startup_diagnostic};
    use crate::config::Config;
    use std::collections::HashMap;

    #[test]
    fn startup_diagnostic_does_not_echo_supplied_configuration_values() {
        let values = HashMap::from([
            ("BUNGIE_API_KEY".into(), "secret-api-key".into()),
            ("BUNGIE_PUBLIC_CLIENT_ID".into(), "secret-client-id".into()),
            (
                "BUNGIE_CALLBACK_URI".into(),
                "http://example.test:43127/callback?code=secret".into(),
            ),
            (
                "BUNGIE_REGISTERED_CALLBACK_URI".into(),
                "http://example.test:43127/callback?code=secret".into(),
            ),
        ]);
        let error = match Config::from_values(&values) {
            Ok(_) => panic!("invalid configuration should fail"),
            Err(error) => error,
        };

        let diagnostic = startup_diagnostic(error).to_string();
        assert_eq!(
            diagnostic,
            "error: required runtime configuration is missing or invalid"
        );
        for supplied in [
            "secret-api-key",
            "secret-client-id",
            "example.test",
            "secret",
        ] {
            assert!(
                !diagnostic.contains(supplied),
                "diagnostic leaked {supplied}"
            );
        }
    }

    #[test]
    fn startup_diagnostic_accepts_only_sanitized_application_errors() {
        assert_eq!(
            startup_diagnostic(AppError::configuration()).to_string(),
            "error: required runtime configuration is missing or invalid"
        );
    }
}
