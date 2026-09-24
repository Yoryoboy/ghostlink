//! Opaque identifiers shared by the provider-neutral domain.

use crate::error::{AppError, ErrorCategory};
use std::fmt;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(crate::redact::Redacted<String>);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(crate::redact::Redacted::new(value.into()))
            }

            pub fn as_str(&self) -> &str {
                self.0.expose_secret()
            }

            pub fn into_inner(self) -> String {
                self.0.into_inner()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                crate::redact::opaque(formatter, stringify!($name))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(MembershipId);
string_id!(CharacterId);
string_id!(ItemInstanceId);
string_id!(DefinitionVersion);
string_id!(PlanId);

/// A non-secret request-correlation identifier.
///
/// Unlike the opaque identifiers in this module, a correlation identifier is
/// deliberately safe to render: it exists to tie a set of diagnostics together,
/// and hiding it would defeat that purpose. It is still bounded to visible
/// ASCII so it can never inject a control character, a line break, or a header
/// separator when it is carried through a diagnostic, and it must never be
/// derived from a provider payload, credential, or account value.
///
/// [`fmt::Display`] renders the bare value and [`fmt::Debug`] renders
/// `CorrelationId(value)`, both intentionally unredacted.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// The longest correlation identifier a diagnostic may carry.
    pub const MAX_LEN: usize = 64;

    /// Validates a candidate correlation identifier.
    ///
    /// Empty, oversized, whitespace-bearing, control-bearing, or non-ASCII
    /// values are rejected as a sanitized configuration failure; the rejected
    /// value never appears in the error.
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_LEN
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(AppError::new(ErrorCategory::ConfigurationError));
        }
        Ok(Self(value))
    }

    /// The correlation value as a printable string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "CorrelationId({})", self.0)
    }
}

/// A provider membership type kept open because providers may add values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MembershipType(u32);

impl MembershipType {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for MembershipType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A definition hash. It is deliberately distinct from an item instance ID.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemHash(i64);

impl ItemHash {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i64 {
        self.0
    }
}

impl fmt::Display for ItemHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A perk definition hash. It is deliberately distinct from an [`ItemHash`]
/// and a [`BucketHash`], because a perk reference must never be confused with an
/// item or bucket reference even though providers may reuse numeric spaces.
///
/// The width is `u32` rather than the signed `i64` used by item and bucket
/// hashes: this is a provider-neutral perk identifier, and keeping it unsigned
/// and narrow makes the durable projection's integer range check exact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PerkHash(u32);

impl PerkHash {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PerkHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BucketHash(i64);

impl BucketHash {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i64 {
        self.0
    }
}

impl fmt::Display for BucketHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An opaque revision assigned by the snapshot repository.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SyncRevision(u64);

impl SyncRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SyncRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CharacterId, CorrelationId, ItemHash, ItemInstanceId, MembershipId, MembershipType,
        PerkHash, SyncRevision,
    };
    use crate::error::ErrorCategory;

    #[test]
    fn identifiers_are_explicitly_constructed_and_read() {
        assert_eq!(CharacterId::new("character").as_str(), "character");
        assert_eq!(MembershipType::new(42).value(), 42);
        assert_eq!(SyncRevision::new(7).value(), 7);
    }

    #[test]
    fn definition_hash_and_instance_id_have_no_cross_conversion() {
        let definition = ItemHash::new(123);
        let instance = ItemInstanceId::new("123");
        assert_eq!(definition.value(), 123);
        assert_eq!(instance.as_str(), "123");
    }

    #[test]
    fn perk_hash_is_unsigned_and_distinct_from_item_and_bucket_hashes() {
        assert_eq!(PerkHash::new(0).value(), 0);
        assert_eq!(PerkHash::new(u32::MAX).value(), u32::MAX);
        assert_eq!(PerkHash::new(u32::MAX).to_string(), "4294967295");

        // The types are not interchangeable: the same numeric value in an item
        // hash and a perk hash are different values with no cross conversion.
        let item = ItemHash::new(7);
        let perk = PerkHash::new(7);
        assert_eq!(item.value(), i64::from(perk.value()));
    }

    #[test]
    fn string_backed_identifier_debug_is_redacted() {
        let membership = MembershipId::new("membership-secret-sentinel");
        let character = CharacterId::new("character-secret-sentinel");
        let item = ItemInstanceId::new("item-secret-sentinel");

        let membership_debug = format!("{membership:?}");
        let character_debug = format!("{character:?}");
        let item_debug = format!("{item:?}");

        assert_eq!(membership_debug, "MembershipId");
        assert_eq!(character_debug, "CharacterId");
        assert_eq!(item_debug, "ItemInstanceId");
        assert!(!membership_debug.contains("membership-secret-sentinel"));
        assert!(!character_debug.contains("character-secret-sentinel"));
        assert!(!item_debug.contains("item-secret-sentinel"));
    }

    #[test]
    fn raw_identifier_access_remains_explicit() {
        let character = CharacterId::new("character-secret-sentinel");
        let item = ItemInstanceId::new("item-secret-sentinel");

        assert_eq!(character.as_str(), "character-secret-sentinel");
        assert_eq!(item.into_inner(), "item-secret-sentinel");
    }

    #[test]
    fn correlation_id_is_printable_and_bounded_to_visible_ascii() {
        let id = CorrelationId::new("req-2026-09-24-0001").expect("valid correlation id");
        // A correlation id is intentionally non-secret: both formatting paths
        // render the value so diagnostics can group by request.
        assert_eq!(id.as_str(), "req-2026-09-24-0001");
        assert_eq!(id.to_string(), "req-2026-09-24-0001");
        assert_eq!(format!("{id:?}"), "CorrelationId(req-2026-09-24-0001)");

        // The exact length bound is inclusive, and every byte is visible ASCII
        // so a correlation id can never smuggle a control byte or separator.
        let boundary = CorrelationId::new("a".repeat(CorrelationId::MAX_LEN)).expect("boundary");
        assert_eq!(boundary.as_str().len(), CorrelationId::MAX_LEN);
        assert!(
            boundary
                .as_str()
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte))
        );
    }

    #[test]
    fn correlation_id_rejects_unsafe_values_without_echoing_them() {
        let oversized = "a".repeat(CorrelationId::MAX_LEN + 1);
        for invalid in [
            "",
            " ",
            "with space",
            "line\nbreak",
            "tab\t",
            "caf\u{e9}",
            oversized.as_str(),
        ] {
            let error = CorrelationId::new(invalid).expect_err("must reject");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        }

        // A distinctive rejected value never reaches the sanitized diagnostic.
        let sentinel = "correlation-secret-sentinel";
        let error = CorrelationId::new(sentinel.repeat(4)).expect_err("oversized");
        assert!(!format!("{error} {error:?}").contains(sentinel));
    }
}
