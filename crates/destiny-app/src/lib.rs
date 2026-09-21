//! Application-facing configuration, services, and sanitized diagnostics.

mod config;

pub use destiny_core::{AppError, ErrorCategory};

/// Reports that the application foundation is available.
pub fn foundation_ready() -> bool {
    destiny_core::foundation_ready()
}

/// Loads and validates required runtime configuration for application internals.
///
/// This remains crate-visible so future application services can consume the
/// validated configuration without exposing provider credentials to adapters.
pub(crate) fn load_config() -> Result<config::Config, AppError> {
    config::Config::from_env()
}

/// Validates startup configuration and returns only a sanitized application
/// error on failure.
pub fn startup() -> Result<(), AppError> {
    load_config().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{AppError, startup};
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

        let diagnostic = error.to_string();
        assert_eq!(
            diagnostic,
            "required runtime configuration is missing or invalid"
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
    fn startup_exposes_only_the_sanitized_result_type() {
        let startup_fn: fn() -> Result<(), AppError> = startup;
        let _ = startup_fn;
    }
}
