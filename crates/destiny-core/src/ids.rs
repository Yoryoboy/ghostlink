//! Opaque identifiers shared by the provider-neutral domain.

use std::fmt;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(stringify!($name))
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
        CharacterId, ItemHash, ItemInstanceId, MembershipId, MembershipType, SyncRevision,
    };

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
}
