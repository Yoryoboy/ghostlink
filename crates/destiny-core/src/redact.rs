//! Shared, provider-free redaction primitives.
//!
//! Every secret-bearing or value-hiding type in the workspace formats through
//! this module, so the decision to never render a protected value lives in one
//! place and the redaction marker is a single constant. Three shapes are
//! covered:
//!
//! - [`Redacted`] wraps a secret-bearing value and renders only [`REDACTED`]
//!   from every formatting path; the value is reachable only through
//!   [`Redacted::expose_secret`].
//! - [`redact_optional`] redacts an optional secret reference while keeping its
//!   presence bit, so a diagnostic still distinguishes "present" from
//!   "absent" without exposing the value.
//! - [`opaque`] renders only the stable type name of an opaque identifier or
//!   secret newtype, for types whose sanitized contract is their type name
//!   rather than the shared marker.
//!
//! Nothing here decides *what* is secret; it only makes the formatting
//! contract uniform. A non-secret diagnostic identity such as a correlation id
//! must not be routed through this module, because hiding it would defeat its
//! purpose.

use std::fmt;

/// The single marker rendered in place of any redacted value.
pub const REDACTED: &str = "<redacted>";

/// Writes the redaction marker.
pub fn redacted(formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(REDACTED)
}

/// Writes the stable, non-secret label of a value-hiding type.
///
/// Opaque provider identifiers and secret-bearing newtypes render only their
/// type name; routing them through one function keeps that sanitized decision
/// in a single place.
pub fn opaque(formatter: &mut fmt::Formatter<'_>, label: &str) -> fmt::Result {
    formatter.write_str(label)
}

/// Renders an optional secret reference as the redaction marker while keeping
/// the presence bit.
///
/// `None` stays `None` and `Some(_)` becomes `Some("<redacted>")`, so a
/// diagnostic can still tell whether a credential was supplied without ever
/// seeing its value.
pub fn redact_optional<T>(value: Option<T>) -> Option<&'static str> {
    value.map(|_| REDACTED)
}

/// A secret-bearing value whose formatting paths render only [`REDACTED`].
///
/// The wrapped value is reachable only through the explicit
/// [`Redacted::expose_secret`] accessor, so a trusted adapter can read it while
/// a derived or hand-written diagnostic cannot render it.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a secret-bearing value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Exposes the secret to a trusted caller.
    pub fn expose_secret(&self) -> &T {
        &self.0
    }

    /// Consumes the wrapper and returns the secret to a trusted caller.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        redacted(formatter)
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        redacted(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::{REDACTED, Redacted, opaque, redact_optional};
    use std::fmt;

    const SENTINEL: &str = "redaction-sentinel-value";

    #[test]
    fn redacted_wrapper_hides_the_value_from_debug_and_display() {
        let secret = Redacted::new(SENTINEL);
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{secret}"), REDACTED);
        assert!(!format!("{secret:?} {secret}").contains(SENTINEL));
        // The trusted accessor is the only way back to the value.
        assert_eq!(secret.expose_secret(), &SENTINEL);
        assert_eq!(Redacted::new(SENTINEL).into_inner(), SENTINEL);
    }

    #[test]
    fn redact_optional_preserves_presence_without_the_value() {
        assert_eq!(redact_optional::<&str>(None), None);
        let present = Some(SENTINEL);
        let rendered = format!("{:?}", redact_optional(present));
        assert_eq!(rendered, format!("Some({REDACTED:?})"));
        assert!(!rendered.contains(SENTINEL));
    }

    struct Labeled;

    impl fmt::Debug for Labeled {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            opaque(formatter, "Labeled")
        }
    }

    #[test]
    fn opaque_prints_only_the_label() {
        assert_eq!(format!("{:?}", Labeled), "Labeled");
    }
}
