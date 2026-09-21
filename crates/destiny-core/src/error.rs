//! Stable, adapter-neutral application error categories.

use std::fmt;

/// Categories adapters can safely use when presenting failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCategory {
    AuthorizationRequired,
    ReauthorizationRequired,
    AuthorizationDenied,
    CsrfStateMismatch,
    CredentialStoreUnavailable,
    ProviderAuthentication,
    ProviderRateLimit,
    ProviderUnavailable,
    ProviderValidation,
    StaleState,
    AmbiguousMatch,
    UnsupportedOperation,
    PartialFailure,
    VerificationFailed,
    LocalStorage,
    ConfigurationError,
    ClockOverflow,
    PlanIdExhausted,
}

/// An application failure whose presentation is intentionally sanitized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppError {
    category: ErrorCategory,
}

impl AppError {
    pub const fn new(category: ErrorCategory) -> Self {
        Self { category }
    }

    pub const fn configuration() -> Self {
        Self::new(ErrorCategory::ConfigurationError)
    }

    pub const fn category(self) -> ErrorCategory {
        self.category
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.category {
            ErrorCategory::AuthorizationRequired => "authorization is required",
            ErrorCategory::ReauthorizationRequired => "reauthorization is required",
            ErrorCategory::AuthorizationDenied => "authorization was denied",
            ErrorCategory::CsrfStateMismatch => "authorization state was rejected",
            ErrorCategory::CredentialStoreUnavailable => "credential storage is unavailable",
            ErrorCategory::ProviderAuthentication => "provider authentication failed",
            ErrorCategory::ProviderRateLimit => "the provider rate limit was reached",
            ErrorCategory::ProviderUnavailable => "the provider is unavailable",
            ErrorCategory::ProviderValidation => "the provider rejected the request",
            ErrorCategory::StaleState => "local state is stale",
            ErrorCategory::AmbiguousMatch => "the request matched multiple items",
            ErrorCategory::UnsupportedOperation => "the operation is unsupported",
            ErrorCategory::PartialFailure => "the operation completed only partially",
            ErrorCategory::VerificationFailed => "the operation could not be verified",
            ErrorCategory::LocalStorage => "local storage failed",
            ErrorCategory::ConfigurationError => {
                "required runtime configuration is missing or invalid"
            }
            ErrorCategory::ClockOverflow => "clock value exceeded its supported range",
            ErrorCategory::PlanIdExhausted => "plan identifiers are exhausted",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod tests {
    use super::{AppError, ErrorCategory};

    #[test]
    fn configuration_error_is_stable_and_sanitized() {
        let error = AppError::configuration();
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        assert_eq!(
            error.to_string(),
            "required runtime configuration is missing or invalid"
        );
        assert!(!error.to_string().contains("API"));
    }
}
