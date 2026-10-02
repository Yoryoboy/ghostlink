//! Production cryptographic authorization-state generation.
//!
//! [`SystemSecureRandom`] is the only production implementation of the
//! [`SecureRandom`] port. It draws from the operating-system CSPRNG through
//! `getrandom`, so authorization state is unpredictable instead of being derived
//! from a clock, a process identifier, an address, or any other guessable input.
//!
//! The drawn bytes are hex-encoded before they become an [`AuthorizationState`]
//! because the state is placed verbatim in the authorization URL query. Hex keeps
//! the value valid UTF-8, URL-safe, and exactly two ASCII characters per random
//! byte, so the later encoding step in
//! [`authorization_url`](crate::oauth::authorization_url) can never fail and no
//! byte can be reinterpreted or dropped by a URL library.

use destiny_core::{AppError, AuthorizationState, ErrorCategory, SecureRandom};

/// The number of random bytes behind one authorization state (256 bits).
const STATE_BYTES: usize = 32;

/// The lowercase hexadecimal alphabet used to encode the drawn bytes.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The production [`SecureRandom`] over the operating-system CSPRNG.
///
/// It holds no state and is cheap to construct, so a composition root builds one
/// per login attempt instead of threading a generator through the application.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSecureRandom;

impl SystemSecureRandom {
    /// Builds the generator.
    ///
    /// Construction is infallible; an unavailable OS entropy source is reported
    /// per [`SecureRandom::generate_state`] call.
    pub const fn new() -> Self {
        Self
    }
}

impl SecureRandom for SystemSecureRandom {
    /// Generates one unpredictable, URL-safe authorization state.
    ///
    /// # Errors
    ///
    /// An operating-system entropy failure is reported as the sanitized
    /// [`ErrorCategory::LocalStorage`] category: the entropy source is a local
    /// machine facility, not a provider call, and no operating-system error text,
    /// path, or value is ever rendered.
    fn generate_state(&mut self) -> Result<AuthorizationState, AppError> {
        let mut bytes = [0u8; STATE_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        let mut encoded = Vec::with_capacity(STATE_BYTES * 2);
        for byte in bytes {
            encoded.push(HEX[usize::from(byte >> 4)]);
            encoded.push(HEX[usize::from(byte & 0x0f)]);
        }
        Ok(AuthorizationState::from_bytes(encoded))
    }
}

#[cfg(test)]
mod tests {
    use super::{HEX, STATE_BYTES, SystemSecureRandom};
    use destiny_core::SecureRandom;

    #[test]
    fn generates_url_safe_ascii_state_of_the_expected_length() {
        let mut generator = SystemSecureRandom::new();
        let state = generator
            .generate_state()
            .expect("the operating-system entropy source reads");

        let bytes = state.as_bytes();
        assert_eq!(bytes.len(), STATE_BYTES * 2);
        assert!(bytes.iter().all(|byte| HEX.contains(byte)));
        assert!(std::str::from_utf8(bytes).is_ok());
    }

    #[test]
    fn successive_states_are_different() {
        // The state is a CSRF nonce, so repeating one would let a replayed
        // callback satisfy the comparison. Two draws are independent.
        let mut generator = SystemSecureRandom::new();
        let first = generator
            .generate_state()
            .expect("the entropy source reads");
        let second = generator
            .generate_state()
            .expect("the entropy source reads");
        assert_ne!(first, second);
    }

    #[test]
    fn the_generated_state_is_never_rendered_by_debug() {
        let mut generator = SystemSecureRandom::new();
        let state = generator
            .generate_state()
            .expect("the entropy source reads");
        assert_eq!(format!("{state:?}"), "AuthorizationState");
    }

    #[test]
    fn a_generated_state_survives_the_authorization_url_query() {
        // The state is placed verbatim in the authorization query, so a generated
        // value must be valid UTF-8 and must round-trip through URL encoding
        // unchanged; otherwise login would fail on a value nobody can predict.
        let mut generator = SystemSecureRandom::new();
        let state = generator
            .generate_state()
            .expect("the entropy source reads");
        let url = crate::oauth::authorization_url(
            "public-client-id",
            "http://127.0.0.1:43127/oauth/callback",
            &state,
        )
        .expect("a generated state is valid UTF-8 for the query");

        let carried = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("the authorization query carries the state");
        assert_eq!(carried.as_bytes(), state.as_bytes());
    }

    #[test]
    fn the_port_implementation_is_object_safe_for_composition() {
        let _: Box<dyn SecureRandom> = Box::new(SystemSecureRandom::new());
    }
}
