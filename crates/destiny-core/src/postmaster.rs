//! Committed Postmaster observations, not a live read or a pull contract.
//!
//! Each character group is explicitly its owner's Postmaster location/container.
//! Instances retain genuine provider IDs; stacks are observations with no action
//! selector, row identity, or cross-refresh identity. Equal stack rows remain
//! separate: quantities must never be summed or deduplicated by this projection.
//!
//! Instance runtime buckets were not retained in the snapshot and are omitted;
//! a definition bucket cannot substitute for one. Stack buckets are the actual
//! runtime observations. Optional names enrich holdings without erasing unknown
//! definitions. Metadata describes the committed profile snapshot, not a promise
//! of current live holdings or complete Postmaster acquisition. Currency coverage
//! qualifies component 103 only, including legacy NotObserved snapshots; neither
//! Available currencies nor empty character groups establish Postmaster coverage.

use crate::{
    BucketHash, CharacterId, ItemHash, ItemInstanceId, ProfileCurrenciesCoverage, QueryMetadata,
};

/// One synchronized committed read, including owned characters with no holdings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostmasterResult {
    pub metadata: QueryMetadata,
    pub profile_currencies_coverage: ProfileCurrenciesCoverage,
    /// Ascending character ID order, independent of repository iteration order.
    pub characters: Vec<CharacterPostmaster>,
}

/// Both arrays belong exclusively to this character's Postmaster.
/// No carried inventory, Vault holdings, or currency summaries appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterPostmaster {
    pub character_id: CharacterId,
    pub class: Option<String>,
    /// Unequipped instances ordered by genuine instance ID.
    pub instances: Vec<PostmasterInstance>,
    /// Ordered by item hash, runtime bucket, then quantity; duplicates survive.
    pub stacks: Vec<PostmasterStack>,
}

/// A genuine instanced holding in the enclosing owner's Postmaster location.
/// The ID is real, but this read alone does not authorize any mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostmasterInstance {
    pub instance_id: ItemInstanceId,
    pub item_hash: ItemHash,
    pub name: Option<String>,
    pub quantity: u32,
}

/// An anonymous observation in the enclosing owner's Postmaster container.
/// Neither the row's contents nor its sorted position is an action selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostmasterStack {
    pub item_hash: ItemHash,
    pub name: Option<String>,
    /// Actual runtime bucket, never substituted from a definition.
    pub bucket_hash: BucketHash,
    pub quantity: i32,
}
