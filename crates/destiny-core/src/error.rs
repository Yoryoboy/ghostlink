//! Stable, adapter-neutral application error categories.

use std::fmt;

/// Categories adapters can safely use when presenting failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCategory {
    AuthorizationRequired,
    ReauthorizationRequired,
    AlreadyAuthorizing,
    AlreadyAuthorized,
    AuthorizationDenied,
    CsrfStateMismatch,
    CredentialStoreUnavailable,
    ProviderAuthentication,
    ProviderRateLimit,
    ProviderUnavailable,
    ProviderValidation,
    DefinitionsUnavailable,
    StaleState,
    AmbiguousMatch,
    UnsupportedOperation,
    PartialFailure,
    VerificationFailed,
    LocalStorage,
    ConfigurationError,
    CallbackTimeout,
    ClockOverflow,
    PlanIdExhausted,
}

impl ErrorCategory {
    /// True only for the explicit provider rate-limit refusal.
    ///
    /// A rate limit is a transient throttle: the provider accepted the request
    /// shape and only asks the caller to slow down, so an idempotent read may be
    /// repeated under a bounded pacing policy. It is never a reason to repeat a
    /// mutation step, which stays one-shot and intent-bound.
    pub const fn is_rate_limit(self) -> bool {
        matches!(self, Self::ProviderRateLimit)
    }

    /// True for a transient provider condition that is safe to repeat for a
    /// read: the explicit rate-limit refusal and a provider-unavailable failure.
    ///
    /// This is the broadest classification a bounded read-retry policy may use.
    /// Authorization, validation, ambiguity, and verification categories are
    /// deliberately excluded, so a retry can never mask an outcome that needs a
    /// human decision.
    pub const fn is_retryable_read(self) -> bool {
        matches!(self, Self::ProviderRateLimit | Self::ProviderUnavailable)
    }
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
            ErrorCategory::AlreadyAuthorizing => "authorization is already in progress",
            ErrorCategory::AlreadyAuthorized => "authorization is already active",
            ErrorCategory::AuthorizationDenied => "authorization was denied",
            ErrorCategory::CsrfStateMismatch => "authorization state was rejected",
            ErrorCategory::CredentialStoreUnavailable => "credential storage is unavailable",
            ErrorCategory::ProviderAuthentication => "provider authentication failed",
            ErrorCategory::ProviderRateLimit => "the provider rate limit was reached",
            ErrorCategory::ProviderUnavailable => "the provider is unavailable",
            ErrorCategory::ProviderValidation => "the provider rejected the request",
            ErrorCategory::DefinitionsUnavailable => {
                "definitions not loaded; run ghostlink sync-manifest"
            }
            ErrorCategory::StaleState => "local state is stale",
            ErrorCategory::AmbiguousMatch => "the request matched multiple items",
            ErrorCategory::UnsupportedOperation => "the operation is unsupported",
            ErrorCategory::PartialFailure => "the operation completed only partially",
            ErrorCategory::VerificationFailed => "the operation could not be verified",
            ErrorCategory::LocalStorage => "local storage failed",
            ErrorCategory::ConfigurationError => {
                "required runtime configuration is missing or invalid"
            }
            ErrorCategory::CallbackTimeout => "the authorization callback timed out",
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
