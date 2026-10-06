//! Minimal provider-neutral behavioral ports.

use crate::auth::{
    AccessCredential, AuthorizationCallback, AuthorizationCode, AuthorizationState, RefreshToken,
};
use crate::domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, SyncMetadata,
};
use crate::error::{AppError, ErrorCategory};
use crate::ids::{
    BucketHash, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId, MembershipType, PlanId,
};
use crate::query::{DefinitionSearchQuery, InstanceSearchQuery};
use crate::state::{ProfileSyncReason, Resolution, SourceMarker, Timestamp};
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

/// Supplies application time without coupling the core to a clock runtime.
pub trait Clock {
    fn now(&self) -> Result<Timestamp, AppError>;
}

/// Supplies opaque plan identifiers without prescribing an ID algorithm.
/// Generates unpredictable, opaque authorization state values.
pub trait SecureRandom {
    fn generate_state(&mut self) -> Result<AuthorizationState, AppError>;
}

/// A provider-neutral request to begin browser authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationRequest {
    pub state: AuthorizationState,
    pub expires_at: Timestamp,
}

pub trait BrowserAuthorization {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError>;
}

pub trait CallbackReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError>;
}

/// Exchanges a code for access material without exposing client configuration.
///
/// Public mode returns access-only credentials; confidential mode may also
/// return a refresh grant. The historical trait name is retained for compatibility.
pub trait PublicTokenExchange {
    fn exchange(&mut self, code: AuthorizationCode) -> Result<AccessCredential, AppError>;
}

/// Exchanges opaque refresh material for a new access credential.
///
/// A successful response may omit a refresh grant; the caller must then retain
/// the previous grant. A returned grant replaces the previous one (rotation).
/// Provider rejection with HTTP 400 or 401 maps to
/// [`ErrorCategory::ReauthorizationRequired`], never to raw provider text.
pub trait TokenRefresh {
    fn refresh(&mut self, token: &RefreshToken) -> Result<AccessCredential, AppError>;
}

/// Supplies a currently valid access credential for each provider request.
///
/// Refreshing implementations own expiry policy and synchronize refresh/store
/// mutation internally. Shared access lets multiple gateways use one lock-backed
/// source without requiring exclusive access to the source itself.
pub trait CredentialSource {
    fn current(&self) -> Result<AccessCredential, AppError>;
}

/// Fixed credential source for access-only compositions and deterministic tests.
///
/// Always returns a clone without checking time or refreshing. The caller is
/// responsible for supplying a credential valid for its intended requests.
#[derive(Clone, Debug)]
pub struct StaticCredentialSource {
    credential: AccessCredential,
}

impl StaticCredentialSource {
    pub fn new(credential: AccessCredential) -> Self {
        Self { credential }
    }
}

impl CredentialSource for StaticCredentialSource {
    fn current(&self) -> Result<AccessCredential, AppError> {
        Ok(self.credential.clone())
    }
}

pub trait PlanIdGenerator {
    fn next_plan_id(&mut self) -> Result<PlanId, AppError>;
}

/// A normalized inventory value before repository-owned revision assignment.
///
/// `checked_at` and `source` are freshness and provenance metadata, not
/// normalized projection content. Revision identity therefore ignores
/// `checked_at`, and it ignores `source` once the fixed dataset/source pairing
/// has been validated. An identical refresh at a later `checked_at` retains the
/// revision and refreshes the committed timestamp. `membership_id`,
/// `membership_type`, `definition_version`, and the character and item records
/// remain part of revision identity, so any field change advances the revision
/// exactly once. Characters and items are unordered sets keyed by their record
/// identifiers, so a pure permutation of either collection is not a content
/// change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySnapshotInput {
    pub plug_set_entries: Vec<crate::domain::PlugSetEntry>,
    pub membership_id: MembershipId,
    pub membership_type: MembershipType,
    pub definition_version: DefinitionVersion,
    pub checked_at: Timestamp,
    pub source: SourceMarker,
    pub characters: Vec<Character>,
    pub items: Vec<ItemInstance>,
}

impl InventorySnapshotInput {
    pub fn with_metadata(snapshot: &InventorySnapshot) -> Self {
        Self {
            membership_id: snapshot.metadata.membership_id.clone(),
            membership_type: snapshot.metadata.membership_type,
            definition_version: snapshot.metadata.definition_version.clone(),
            checked_at: snapshot.metadata.checked_at,
            source: snapshot.metadata.source,
            characters: snapshot.characters.clone(),
            items: snapshot.items.clone(),
            plug_set_entries: snapshot.plug_set_entries.clone(),
        }
    }

    /// Compares the normalized projection content that determines revision identity.
    ///
    /// `checked_at` and `source` are deliberately excluded: freshness alone never
    /// advances a revision, and `source` is validated against the dataset before
    /// a replacement is accepted. Characters and items are compared as unordered
    /// sets keyed by their record identifiers: both sides must carry the same
    /// complete records, but a pure permutation of either collection is not a
    /// content change. Records are sorted by identifier and compared in full
    /// without hashing, so every [`Character`] field and every [`ItemInstance`]
    /// field, including the typed [`ItemLocation`], participates in equality.
    ///
    /// An [`ItemInstance`]'s `perks` vector is compared in its stored order, so
    /// every [`PerkSummary`] field and the exact perk order participate in
    /// identity: a reordered perk list is a content change, while a pure
    /// permutation of the *items* themselves is not. Stat values participate in
    /// identity, but stat order does not: both sides are sorted by stat hash.
    /// Plug-set entries are unordered, keyed by scope and hash; both flags
    /// participate in content identity independently.
    ///
    /// [`PerkSummary`]: crate::domain::PerkSummary
    pub fn content_eq(&self, other: &Self) -> bool {
        if self.membership_id != other.membership_id
            || self.membership_type != other.membership_type
            || self.definition_version != other.definition_version
            || self.characters.len() != other.characters.len()
            || self.items.len() != other.items.len()
        {
            return false;
        }

        let mut left_characters: Vec<&Character> = self.characters.iter().collect();
        let mut right_characters: Vec<&Character> = other.characters.iter().collect();
        left_characters.sort_by(|left, right| left.id.cmp(&right.id));
        right_characters.sort_by(|left, right| left.id.cmp(&right.id));
        if left_characters != right_characters {
            return false;
        }

        let mut left_items = self.items.clone();
        let mut right_items = other.items.clone();
        for item in left_items.iter_mut().chain(right_items.iter_mut()) {
            item.stats.sort_by_key(|stat| stat.stat_hash);
        }
        left_items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        right_items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let mut left_options = self.plug_set_entries.clone();
        let mut right_options = other.plug_set_entries.clone();
        left_options.sort();
        right_options.sort();
        left_items == right_items && left_options == right_options
    }

    /// Reports whether the character and item collections form a complete,
    /// internally consistent projection, independent of provenance.
    ///
    /// Character identifiers and item instance identifiers must each be unique,
    /// and every [`ItemLocation::Character`] must reference a character present
    /// in this same input. Empty character and item vectors are structurally
    /// valid. This method deliberately does not validate `source`: requiring the
    /// fixed dataset/source pairing is repository-specific policy, so a
    /// repository validates provenance separately before mutating state.
    ///
    /// An [`ItemInstance`]'s `perks` vector has no unique key and every one of
    /// its fields is already a valid typed value, so structural validity imposes
    /// no additional restriction on it: duplicated perk hashes and any perk
    /// order are structurally valid, and they are left to revision identity and
    /// the durable repository's own storage constraints. Stats have unique hashes
    /// and at most 64 entries per item; incoming order need not be canonical.
    /// Plug-set entries must have unique scoped u32 hashes, and character scopes
    /// must be canonical decimal identifiers referencing this projection.
    pub fn is_structurally_valid(&self) -> bool {
        let mut character_ids = HashSet::with_capacity(self.characters.len());
        for character in &self.characters {
            if !character_ids.insert(character.id.clone()) {
                return false;
            }
        }

        let mut option_keys = HashSet::new();
        if self.plug_set_entries.len() > 4096 * 512 {
            return false;
        }
        for entry in &self.plug_set_entries {
            if u32::try_from(entry.plug_hash.value()).is_err()
                || !option_keys.insert((&entry.scope, entry.plug_hash))
            {
                return false;
            }
            if let crate::domain::PlugSetScope::Character(id) = &entry.scope {
                let text = id.as_str();
                if !character_ids.contains(id)
                    || text.is_empty()
                    || text.len() > 128
                    || !text.bytes().all(|byte| byte.is_ascii_digit())
                    || (text.len() > 1 && text.starts_with('0'))
                {
                    return false;
                }
            }
        }

        let mut instance_ids = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            if !instance_ids.insert(item.instance_id.clone()) {
                return false;
            }
        }

        for item in &self.items {
            let mut hashes = HashSet::new();
            if item.stats.len() > 64 || item.stats.iter().any(|stat| !hashes.insert(stat.stat_hash))
            {
                return false;
            }
            let mut sockets = std::collections::BTreeMap::new();
            let mut previous = None;
            let mut available_hashes = HashSet::new();
            for plug in &item.available_plugs {
                if previous.is_some_and(|index| index > plug.socket_index)
                    || u32::try_from(plug.plug_hash.value()).is_err()
                    || !available_hashes.insert((plug.socket_index, plug.plug_hash))
                {
                    return false;
                }
                previous = Some(plug.socket_index);
                let count = sockets.entry(plug.socket_index).or_insert(0);
                *count += 1;
                if *count > 64 || sockets.len() > 32 {
                    return false;
                }
            }
            if item.plugs.len() > 32
                || item
                    .plugs
                    .iter()
                    .any(|plug| !plug.enabled || u32::try_from(plug.plug_hash.value()).is_err())
            {
                return false;
            }
            if let ItemLocation::Character(character_id) = &item.location
                && !character_ids.contains(character_id)
            {
                return false;
            }
        }
        true
    }
}

/// Reads the active normalized definitions and publishes complete replacements.
///
/// A publishable [`ManifestDefinitions`] set is complete, internally consistent,
/// and revision-free. A [`DefinitionReplacementInput`] adds the committed
/// freshness and provenance markers. Publication replaces every previously
/// visible definition, rejects an invalid source marker, an empty set, duplicate
/// hashes, and mixed definition versions before mutating state, and leaves the
/// prior set and its metadata readable when it fails. The repository owns durable
/// revision allocation: normalized content equality (the complete definition set
/// and its version) retains the revision, any content change advances it exactly
/// once, and a later `checked_at` alone retains the revision while refreshing the
/// committed metadata timestamp.
pub trait DefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError>;
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError>;
    /// Reads the committed metadata of the current definition projection, if any.
    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError>;
    /// Atomically publishes one complete normalized set and returns its committed metadata.
    fn replace_definitions(
        &mut self,
        replacement: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError>;
    /// Item-only acquisition compatibility seam. Implementations that support
    /// tooltips publish both catalogs atomically; unsupported repositories reject
    /// a nonempty catalog rather than silently losing it.
    fn replace_definitions_with_descriptions(
        &mut self,
        replacement: DefinitionReplacementInput,
        descriptions: Vec<crate::ItemDescription>,
    ) -> Result<SyncMetadata, AppError> {
        if !descriptions.is_empty() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        self.replace_definitions(replacement)
    }
}

/// Manifest sync read seam for repositories that expose the complete catalog count.
///
/// Kept separate from definition lookups so read-only composition wrappers need
/// not implement publication summaries. Failures must propagate, not imply zero.
pub trait DefinitionCatalogRepository: DefinitionRepository {
    fn definition_count(&self) -> Result<usize, AppError>;
}

/// World lookups and atomic publication of all four Manifest catalogs.
/// Counts are ordered milestone, activity, activity modifier. Failed publication
/// leaves every catalog and the shared Manifest metadata unchanged.
pub trait WorldDefinitionRepository: DefinitionRepository {
    /// Optional community catalog read, backed by the separate Clarity repository.
    fn lookup_clarity_by_hash(
        &self,
        _hash: u32,
    ) -> Result<Option<crate::ClarityInsight>, AppError> {
        Ok(None)
    }
    fn lookup_clarity_by_name(&self, _name: &str) -> Result<Vec<crate::ClarityInsight>, AppError> {
        Ok(Vec::new())
    }
    /// Reads the official inventory tooltip without changing item identity.
    fn lookup_item_description(&self, hash: ItemHash) -> Result<Option<String>, AppError>;
    fn lookup_milestone(&self, hash: u32) -> Result<Option<crate::MilestoneDefinition>, AppError>;
    fn lookup_activity(&self, hash: u32) -> Result<Option<crate::ActivityDefinition>, AppError>;
    fn lookup_activity_modifier(
        &self,
        hash: u32,
    ) -> Result<Option<crate::ActivityModifierDefinition>, AppError>;
    fn lookup_objective(&self, hash: u32) -> Result<Option<crate::ObjectiveDefinition>, AppError>;
    fn lookup_activity_type(
        &self,
        hash: u32,
    ) -> Result<Option<crate::ActivityTypeDefinition>, AppError>;
    fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError>;
    /// Required weekly catalogs only; loadout names are optional for readiness.
    fn world_definition_counts(&self) -> Result<[usize; 5], AppError>;
    fn publish_manifest(
        &mut self,
        items: DefinitionReplacementInput,
        world: crate::WorldDefinitionsAcquisition,
    ) -> Result<SyncMetadata, AppError>;
}

/// Named stat catalog reads. Name queries use `normalize_stat_name`, match exactly,
/// and return every matching hash in ascending order (including name collisions).
/// Invalid queries fail with ProviderValidation; corrupt storage fails with LocalStorage.
pub trait StatDefinitionRepository {
    fn lookup_stat(&self, hash: u32) -> Result<Option<crate::StatDefinition>, AppError>;
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError>;
    fn stat_definition_count(&self) -> Result<usize, AppError>;
}

/// Reads and atomically replaces the current normalized inventory projection.
pub trait InventoryRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError>;
    /// The repository assigns or retains the revision and publishes only on success.
    fn replace_current(
        &mut self,
        snapshot: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError>;
}

/// Resolves bounded semantic queries against the durable search projection.
///
/// This is the provider-neutral read seam shared by `search_inventory` and
/// `inspect_item`. Every method is a pure read of the caller's committed
/// projection: it performs no synchronization, no provider call, and no
/// mutation, so the application layer alone decides when a projection must be
/// refreshed before a query is answered.
///
/// The contract is deliberately implementable with identical behavior by an
/// in-memory fake and by the durable SQLite adapter, and it assumes no full-text
/// search capability:
///
/// - [`Self::search_definitions`] validates the query, returns every matching
///   [`ItemDefinition`] whose normalized name, type, and tags satisfy it,
///   orders the matches by [`ItemHash`] ascending, and then truncates the
///   ordered list to the query limit. An invalid query fails with the sanitized
///   [`ErrorCategory::ProviderValidation`] category.
/// - [`Self::search_instances`] validates the query, returns every matching
///   [`ItemInstance`] that satisfies the definition-hash, exact [`PerkHash`],
///   and character filters, orders the matches by [`ItemInstanceId`] ascending,
///   and then truncates the ordered list to the query limit.
/// - [`Self::lookup_instance`] resolves one concrete instance by its exact
///   identifier and reports [`Resolution::Missing`], [`Resolution::Unique`], or
///   [`Resolution::Ambiguous`] without ever selecting by a display name. A valid
///   projection carries unique instance identifiers, so `Ambiguous` is
///   unreachable for a well-formed repository: duplicate instance keys are
///   rejected by the profile projection's structural validation and by the
///   durable primary key. The variant is inherited unchanged from the shared
///   [`Resolution`] type used by [`DefinitionRepository::lookup_definition`],
///   so its unbounded vector is not a product-facing candidate list and needs no
///   separate bound here; only the caller-controlled search inputs are bounded.
///
/// Matching never returns a raw provider payload, a credential, or an unbounded
/// record, and no method returns a definition where an instance identity is
/// required or the reverse.
///
/// [`PerkHash`]: crate::ids::PerkHash
/// [`ItemInstanceId`]: crate::ids::ItemInstanceId
pub trait SearchRepository {
    /// Complete display-name catalog for a filter kind, not truncated search hits.
    fn name_catalog(&self, _kind: &str) -> Result<crate::name_matching::NameCatalog, AppError> {
        Err(AppError::new(crate::ErrorCategory::UnsupportedOperation))
    }
    /// Bounded, deterministic definition matches for one validated search.
    fn search_definitions(
        &self,
        query: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError>;

    /// Bounded, deterministic concrete-instance matches for one validated search.
    fn search_instances(&self, query: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError>;

    /// Resolves one concrete instance by exact identifier.
    ///
    /// The lookup is exact and never name-based. A `Resolution::Ambiguous`
    /// result is only reachable from a structurally invalid or test-seeded
    /// projection, because a valid projection carries unique instance
    /// identifiers; the unbounded ambiguity vector is inherited from the shared
    /// [`Resolution`] type rather than a bounded product result.
    fn lookup_instance(
        &self,
        instance_id: &ItemInstanceId,
    ) -> Result<Resolution<ItemInstance>, AppError>;
}

/// A validated, provider-neutral Destiny membership identity.
///
/// Some providers identify an account and a Destiny profile separately. This
/// value is the resolved *effective* Destiny membership: the numeric provider
/// membership type paired with the opaque decimal membership identifier that
/// profile requests need. It carries no provider payload, no display name, and
/// no credential material.
///
/// Construction validates the identifier once. It must be a non-empty run of
/// ASCII decimal digits no longer than the bound the durable projection already
/// accepts, so a value built here can always be persisted without a second
/// shape check. The membership type stays open because providers may add values;
/// it is a `u32` by construction and needs no further validation.
///
/// The identifier is opaque. [`fmt::Debug`] is implemented manually and redacts
/// it, so a raw membership identifier never reaches logs or diagnostics; the
/// membership type remains visible because it is a non-secret numeric value.
/// Callers that genuinely need the identifier use [`Self::membership_id`].
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DestinyMembership {
    membership_type: MembershipType,
    membership_id: MembershipId,
}

impl DestinyMembership {
    /// The longest identifier the durable profile projection accepts.
    const MAX_IDENTIFIER_LEN: usize = 128;

    /// Validates and pairs one membership type with one opaque decimal identifier.
    ///
    /// An empty, oversized, or non-decimal identifier is rejected with a
    /// sanitized [`ErrorCategory::ProviderValidation`] error; the rejected value
    /// never appears in the error.
    ///
    /// [`ErrorCategory::ProviderValidation`]: crate::error::ErrorCategory::ProviderValidation
    pub fn new(
        membership_type: MembershipType,
        membership_id: MembershipId,
    ) -> Result<Self, AppError> {
        let identifier = membership_id.as_str();
        if identifier.is_empty()
            || identifier.len() > Self::MAX_IDENTIFIER_LEN
            || !identifier.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership_type,
            membership_id,
        })
    }

    /// The numeric provider membership type of the effective membership.
    pub const fn membership_type(&self) -> MembershipType {
        self.membership_type
    }

    /// The opaque decimal membership identifier of the effective membership.
    pub const fn membership_id(&self) -> &MembershipId {
        &self.membership_id
    }
}

impl fmt::Debug for DestinyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DestinyMembership")
            .field("membership_type", &self.membership_type)
            .field("membership_id", &crate::redact::REDACTED)
            .finish()
    }
}

/// Resolves the effective Destiny membership and loads its normalized profile.
///
/// Transport, payload, and credential types stop at the adapter. An
/// implementation resolves the authenticated account's single effective
/// membership and then returns one complete normalized inventory candidate for
/// that same membership. The core never receives provider JSON, and neither
/// method exposes a credential value.
///
/// The two methods are deliberately explicit: resolution is a decision with its
/// own failure mode (absent or ambiguous membership), and acquisition is a
/// separate, retryable read. Keeping them separate lets the application resolve
/// once, pass the result to acquisition, and verify that the returned candidate
/// carries that membership identity before any repository mutation.
pub trait ProfileGateway {
    /// Resolves the single effective membership, failing closed when none or several qualify.
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError>;
    /// Loads one complete normalized inventory candidate for the resolved membership.
    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError>;

    /// Resolves the effective membership with the retry scope of one sync reason.
    ///
    /// The default delegates to [`Self::resolve_membership`], whose scope is the
    /// conservative one that never absorbs a transient provider-unavailable
    /// response. An adapter that can select a per-call scope overrides this so an
    /// ordinary read repeats a bounded transient failure while a preflight or
    /// post-action observation stays one-shot. The reason is supplied per call
    /// rather than at construction, because one gateway instance is reused across
    /// ordinary and observation reads.
    fn resolve_membership_scoped(
        &mut self,
        _reason: ProfileSyncReason,
    ) -> Result<DestinyMembership, AppError> {
        self.resolve_membership()
    }

    /// Loads one membership's inventory with the retry scope of one sync reason.
    ///
    /// The default delegates to [`Self::load_inventory`] for the same reason as
    /// [`Self::resolve_membership_scoped`].
    fn load_inventory_scoped(
        &mut self,
        membership: &DestinyMembership,
        _reason: ProfileSyncReason,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.load_inventory(membership)
    }
}

/// A complete versioned normalized definition set published as one replacement.
///
/// It carries no durable revision: normalized equality is deterministic, and the
/// repository owns revision allocation by comparing the complete replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDefinitions {
    pub version: DefinitionVersion,
    pub definitions: Vec<ItemDefinition>,
}

/// A provider-neutral request to replace the complete definition projection.
///
/// It pairs one revision-free [`ManifestDefinitions`] snapshot with the
/// freshness and provenance markers the repository commits atomically. The
/// application service sets the Manifest source itself; a repository rejects a
/// dataset whose source marker does not match. Revision identity is the
/// normalized definition content alone — an unordered complete set with each
/// record compared in full; `checked_at` is committed freshness metadata that
/// never advances a revision by itself, and tag order remains part of identity
/// through each record's ordered `tags` vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionReplacementInput {
    pub definitions: ManifestDefinitions,
    pub checked_at: Timestamp,
    pub source: SourceMarker,
}

impl DefinitionReplacementInput {
    /// Compares the normalized projection content that determines revision identity.
    ///
    /// `checked_at` and `source` are deliberately excluded for the same reason as
    /// [`InventorySnapshotInput::content_eq`]. The definition set is compared as
    /// a set: both sides must carry the exact same version and the same complete
    /// records, but a pure permutation of definitions is not a content change,
    /// because the port publishes a complete set and the durable schema stores
    /// no definition ordinal. Each [`ItemDefinition`] is compared in full, so
    /// tag order inside a record remains part of identity through its ordered
    /// `tags` vector.
    pub fn content_eq(&self, other: &Self) -> bool {
        let left = &self.definitions;
        let right = &other.definitions;
        if left.version != right.version || left.definitions.len() != right.definitions.len() {
            return false;
        }
        let mut left: Vec<&ItemDefinition> = left.definitions.iter().collect();
        let mut right: Vec<&ItemDefinition> = right.definitions.iter().collect();
        left.sort_by_key(|definition| definition.hash);
        right.sort_by_key(|definition| definition.hash);
        left == right
    }
}

/// One untrusted, transport-neutral Manifest definition candidate.
///
/// An adapter maps an upstream payload into this provider-neutral shape, and the
/// application Manifest service alone decides whether the values are bounded,
/// complete, and internally consistent. The record mirrors the normalized
/// [`ItemDefinition`] fields and is identified only by a definition
/// [`ItemHash`]; definition and instance identity are separated by the type
/// boundary, because a runtime instance identifier has no place in a Manifest
/// record and needs no provider metadata to rule out.
#[derive(Clone, Eq, PartialEq)]
pub struct ManifestDefinitionCandidate {
    /// Optional untrusted tooltip text; application validation may omit it.
    pub description: Option<String>,
    /// Ordered, bounded references into the same-version effect catalog.
    pub perk_hashes: Vec<u32>,
    pub hash: ItemHash,
    pub bucket_hash: Option<BucketHash>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    pub element: Option<String>,
    pub damage_type: Option<crate::domain::DamageType>,
    pub slot: Option<crate::domain::ItemSlot>,
    pub class_type: Option<crate::domain::ClassType>,
    pub plug_category: Option<String>,
    pub tags: Vec<String>,
    pub version: DefinitionVersion,
}

impl fmt::Debug for ManifestDefinitionCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestDefinitionCandidate")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

/// One versioned batch of untrusted definition candidates from a Manifest gateway.
///
/// Neither the batch version nor its candidates are trusted: they may be blank,
/// oversized, inconsistent, duplicated, or incomplete. The application Manifest
/// service validates and normalizes the batch before any repository mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestAcquisition {
    pub sandbox_perks: Vec<SandboxPerkCandidate>,
    pub version: DefinitionVersion,
    pub definitions: Vec<ManifestDefinitionCandidate>,
    /// Blank-name or redacted records omitted by the gateway.
    pub skipped_count: usize,
}

/// Untrusted provider-neutral effect text acquired with inventory definitions.
#[derive(Clone, Eq, PartialEq)]
pub struct SandboxPerkCandidate {
    pub hash: u32,
    pub is_displayable: bool,
    pub description: String,
}

impl fmt::Debug for SandboxPerkCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SandboxPerkCandidate")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

/// A small index with an exact version and an adapter-validated component plan.
///
/// The relative path is transport-neutral data, not an absolute URL. Adapters
/// must validate it again on use because callers can construct this value.
#[derive(Clone, Eq, PartialEq)]
pub struct ManifestIndex {
    pub version: DefinitionVersion,
    pub component_path: String,
    pub milestone_path: String,
    pub activity_path: String,
    pub modifier_path: String,
    pub objective_path: String,
    pub activity_type_path: String,
    pub stat_path: String,
    pub loadout_name_path: String,
    pub sandbox_perk_path: String,
}

impl fmt::Debug for ManifestIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestIndex")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// API-key-only acquisition of public milestones, with no account credentials.
/// Uses the same synchronous acquisition style as [`ManifestGateway`].
pub trait PublicMilestonesGateway {
    fn load_public_milestones(&mut self) -> Result<crate::PublicMilestonesSnapshot, AppError>;
}

/// Separates the small version check from acquisition of the large component.
pub trait ManifestGateway {
    fn fetch_index(&mut self) -> Result<ManifestIndex, AppError>;
    fn load_component(&mut self, index: &ManifestIndex) -> Result<ManifestAcquisition, AppError>;
    fn load_world_components(
        &mut self,
        index: &ManifestIndex,
    ) -> Result<crate::WorldDefinitionsAcquisition, AppError>;

    /// Compatibility composition for callers requesting an unconditional acquisition.
    fn load_definitions(&mut self) -> Result<ManifestAcquisition, AppError> {
        let index = self.fetch_index()?;
        self.load_component(&index)
    }
}

/// One provider-neutral request to move one concrete item between two locations.
///
/// This is the whole input of the transfer seam: the effective
/// [`DestinyMembership`], the concrete [`ItemInstanceId`] and its definition
/// [`ItemHash`], the stack quantity, and the two typed endpoints. It deliberately
/// carries no credential, no provider field name, no provider payload, and no
/// pre-encoded destination flag, so a transfer adapter maps [`Self::to_vault`] and
/// the endpoints onto its own request shape and the core never learns a provider
/// parameter name.
///
/// The membership is the complete resolved [`DestinyMembership`], which already
/// pairs the provider membership type with the opaque identifier, so an adapter
/// can issue the request with no out-of-band account input.
///
/// Every field is private and reachable only through its accessor, so a request
/// exists only as the product of [`TransferRequest::new`] and its endpoint policy
/// cannot be bypassed by a struct literal. `quantity` is passed through
/// unchanged, because the application already accepted the provider-reported
/// quantity and this contract must not silently rewrite it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferRequest {
    membership: DestinyMembership,
    item: ItemInstanceId,
    item_hash: ItemHash,
    quantity: u32,
    from: ItemLocation,
    to: ItemLocation,
}

impl TransferRequest {
    /// Builds one request for two distinct Vault/character endpoints.
    ///
    /// An endpoint outside [`ItemLocation::Vault`] and
    /// [`ItemLocation::Character`] is rejected with the sanitized
    /// [`ErrorCategory::UnsupportedOperation`] category, and identical endpoints
    /// are rejected with [`ErrorCategory::ProviderValidation`], because a
    /// transfer to where the item already is has no provider meaning. Neither
    /// rejected value appears in the error.
    ///
    /// [`ErrorCategory::UnsupportedOperation`]: crate::error::ErrorCategory::UnsupportedOperation
    /// [`ErrorCategory::ProviderValidation`]: crate::error::ErrorCategory::ProviderValidation
    pub fn new(
        membership: DestinyMembership,
        item: ItemInstanceId,
        item_hash: ItemHash,
        quantity: u32,
        from: ItemLocation,
        to: ItemLocation,
    ) -> Result<Self, AppError> {
        validate_endpoints(&from, &to)?;
        Ok(Self {
            membership,
            item,
            item_hash,
            quantity,
            from,
            to,
        })
    }

    /// The complete effective membership this request is issued for.
    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }

    /// The concrete item instance to move.
    pub const fn item(&self) -> &ItemInstanceId {
        &self.item
    }

    /// The definition hash of the item to move.
    pub const fn item_hash(&self) -> ItemHash {
        self.item_hash
    }

    /// The stack quantity to move, unchanged from the accepted projection.
    pub const fn quantity(&self) -> u32 {
        self.quantity
    }

    /// The location the item must leave.
    pub const fn from(&self) -> &ItemLocation {
        &self.from
    }

    /// The location the item must reach.
    pub const fn to(&self) -> &ItemLocation {
        &self.to
    }

    /// True when this request moves the item into the Vault.
    pub const fn to_vault(&self) -> bool {
        match self.to {
            ItemLocation::Vault => true,
            ItemLocation::Character(_) | ItemLocation::Other(_) => false,
        }
    }
}

/// Validates the two endpoints of one transfer request.
///
/// Endpoint policy lives here so an adapter never has to decide whether a request
/// is meaningful. Every combination is matched explicitly with no wildcard arm, so
/// adding a location to the domain cannot silently become an accepted transfer
/// endpoint.
fn validate_endpoints(from: &ItemLocation, to: &ItemLocation) -> Result<(), AppError> {
    match (from, to) {
        (
            ItemLocation::Vault | ItemLocation::Character(_),
            ItemLocation::Vault | ItemLocation::Character(_),
        ) => {
            if from == to {
                Err(AppError::new(ErrorCategory::ProviderValidation))
            } else {
                Ok(())
            }
        }
        (ItemLocation::Other(_), _) | (_, ItemLocation::Other(_)) => {
            Err(AppError::new(ErrorCategory::UnsupportedOperation))
        }
    }
}

/// Why the provider deterministically refused one transfer step.
///
/// Every variant is a bounded, sanitized classification of a provider refusal.
/// A provider status string, message, or payload never crosses this seam, so a
/// caller branches on the meaning instead of parsing provider text, and no
/// refusal can smuggle provider copy into a product surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferRejection {
    /// The provider reports the item can never be transferred.
    NotTransferable,
    /// The provider reports the item is equipped and must be unequipped first.
    Equipped,
    /// The provider reports the destination has no room for the item.
    DestinationFull,
    /// The provider rejected the request itself as malformed or unsupported.
    InvalidRequest,
}

impl TransferRejection {
    /// The shared sanitized category for this refusal.
    ///
    /// The two item-specific refusals are unsupported operations for that item.
    /// A full destination and a rejected request are both provider refusals of
    /// the submitted request, so they keep the provider-validation category.
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::NotTransferable | Self::Equipped => ErrorCategory::UnsupportedOperation,
            Self::DestinationFull | Self::InvalidRequest => ErrorCategory::ProviderValidation,
        }
    }
}

/// The sanitized provider result of one transfer step.
///
/// These variants are the only terminal shapes a transfer adapter may report
/// for one request: the step applied, the provider deterministically refused it,
/// the provider failed transiently, or the effect of an already-issued request
/// could not be determined. The last shape is what makes an honest partial
/// result possible; it is never a success and never a clean failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferOutcome {
    /// The provider reported this step applied.
    Transferred,
    /// The provider deterministically refused this step; it changed nothing.
    Rejected(TransferRejection),
    /// Received authentication refusal after dispatch. Category is
    /// `ProviderAuthentication`; the call was attempted but nothing applied.
    AuthenticationRefused,
    /// The provider failed transiently for this step; nothing was applied.
    Transient,
    /// Whether an already-issued step applied could not be determined.
    Uncertain,
}

impl TransferOutcome {
    /// True only when the provider reported the step applied.
    pub const fn applied(self) -> bool {
        match self {
            Self::Transferred => true,
            Self::Rejected(_) | Self::AuthenticationRefused | Self::Transient | Self::Uncertain => {
                false
            }
        }
    }

    /// The bounded refusal, when the provider deterministically refused the step.
    pub const fn rejection(self) -> Option<TransferRejection> {
        match self {
            Self::Rejected(rejection) => Some(rejection),
            Self::Transferred | Self::AuthenticationRefused | Self::Transient | Self::Uncertain => {
                None
            }
        }
    }
}

/// Performs exactly one provider-side item transfer step.
///
/// One call is one step of one already-planned move. The gateway performs no
/// batching, no ordering, no post-condition check, and above all no retry: it
/// issues the single request it is given and reports the sanitized result, so
/// every sequencing, refresh, and retry decision stays in the application
/// executor, which can only attempt a step under a fresh preflight.
///
/// The result contract keeps uncertainty honest:
///
/// - `Ok(TransferOutcome)` classifies a provider response that was observed.
/// - A received auth refusal is `AuthenticationRefused`, preserving both the
///   authentication category and the dispatched-call evidence (not a mutation).
/// - `Err(AppError)` is reserved for a failure before dispatch,
///   such as missing or expired credentials, missing runtime
///   configuration, or a rate-limit or availability failure that happened before
///   the request was dispatched.
/// - A failure after dispatch whose effect cannot be established, including a
///   lost response or an unclassifiable envelope, must be reported as
///   [`TransferOutcome::Uncertain`] instead of an error, because an error would
///   claim the item did not move when that is exactly what is unknown.
///
/// The trait takes the request by reference and returns no raw payload, so an
/// adapter can never hand a credential, a provider body, or a provider message
/// back to the core.
pub trait TransferGateway {
    /// Issues one transfer step and reports its sanitized provider result.
    fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError>;
}

/// A validated, provider-neutral intent to equip one concrete item on a character.
/// Identifier syntax is adapter-owned; empty opaque identifiers are refused here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipItemRequest {
    membership: DestinyMembership,
    item: ItemInstanceId,
    character: crate::CharacterId,
}

impl EquipItemRequest {
    pub fn new(
        membership: DestinyMembership,
        item: ItemInstanceId,
        character: crate::CharacterId,
    ) -> Result<Self, AppError> {
        if item.as_str().is_empty() || character.as_str().is_empty() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            item,
            character,
        })
    }

    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub const fn character(&self) -> &crate::CharacterId {
        &self.character
    }
}

/// Bounded provider refusals; no provider text or identifiers cross the seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipRejection {
    /// The character's current location or online state disallows this action.
    NotInSocialSpace,
    ExoticLimit,
    Unequippable,
    ItemNotFound,
    CharacterNotFound,
    InvalidRequest,
}

impl EquipRejection {
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::NotInSocialSpace | Self::ExoticLimit | Self::Unequippable => {
                ErrorCategory::UnsupportedOperation
            }
            Self::ItemNotFound | Self::CharacterNotFound | Self::InvalidRequest => {
                ErrorCategory::ProviderValidation
            }
        }
    }
}

/// Honest classification of exactly one equip attempt (not verified state).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipOutcome {
    Equipped,
    Rejected(EquipRejection),
    /// Received authentication refusal after dispatch, not an applied mutation.
    /// The category remains `ProviderAuthentication`; pre-dispatch auth is Err.
    AuthenticationRefused,
    /// A deterministic transient refusal: nothing applied.
    Transient,
    /// Dispatch occurred but whether it applied is unknown. Never blindly retry.
    Uncertain,
}

impl EquipOutcome {
    pub const fn applied(self) -> bool {
        matches!(self, Self::Equipped)
    }
    pub const fn rejection(self) -> Option<EquipRejection> {
        match self {
            Self::Rejected(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Issues one equip request with no retry, sequencing, or verification.
/// `Err` is reserved for pre-dispatch failures. Received auth refusals are
/// `Ok(EquipOutcome::AuthenticationRefused)`; post-dispatch ambiguity must be
/// `Ok(EquipOutcome::Uncertain)`.
pub trait EquipGateway {
    fn equip(&mut self, request: &EquipItemRequest) -> Result<EquipOutcome, AppError>;
}

/// A validated, provider-neutral intent to set one concrete item's lock state.
/// Identifier syntax is adapter-owned; empty opaque identifiers are refused here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetLockStateRequest {
    membership: DestinyMembership,
    item: ItemInstanceId,
    character: crate::CharacterId,
    locked: bool,
}

impl SetLockStateRequest {
    pub fn new(
        membership: DestinyMembership,
        item: ItemInstanceId,
        character: crate::CharacterId,
        locked: bool,
    ) -> Result<Self, AppError> {
        if item.as_str().is_empty() || character.as_str().is_empty() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            item,
            character,
            locked,
        })
    }

    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub const fn character(&self) -> &crate::CharacterId {
        &self.character
    }
    pub const fn locked(&self) -> bool {
        self.locked
    }
}

/// Bounded provider refusals; no provider text or identifiers cross the seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockRejection {
    NotInSocialSpace,
    ItemNotFound,
    CharacterNotFound,
    InvalidRequest,
}

impl LockRejection {
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::NotInSocialSpace => ErrorCategory::UnsupportedOperation,
            Self::ItemNotFound | Self::CharacterNotFound | Self::InvalidRequest => {
                ErrorCategory::ProviderValidation
            }
        }
    }
}

/// Honest classification of exactly one lock-state attempt (not verified state).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockOutcome {
    Applied,
    Rejected(LockRejection),
    /// Received authentication refusal after dispatch, not an applied mutation.
    AuthenticationRefused,
    /// A deterministic transient refusal: nothing applied.
    Transient,
    /// Dispatch occurred but whether it applied is unknown. Never blindly retry.
    Uncertain,
}

impl LockOutcome {
    pub const fn applied(self) -> bool {
        matches!(self, Self::Applied)
    }
    pub const fn rejection(self) -> Option<LockRejection> {
        match self {
            Self::Rejected(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Issues one lock-state request with no retry, sequencing, or verification.
/// `Err` is reserved for pre-dispatch failures. Received auth refusals are
/// `Ok(LockOutcome::AuthenticationRefused)`; post-dispatch ambiguity must be
/// `Ok(LockOutcome::Uncertain)`. No raw provider payload crosses this seam.
pub trait LockGateway {
    fn set_lock_state(&mut self, request: &SetLockStateRequest) -> Result<LockOutcome, AppError>;
}

/// Opaque credential material. Bytes are available only through the explicit
/// trusted-adapter exposure boundary and are never available to formatting APIs.
#[derive(Clone, Eq, PartialEq)]
pub struct CredentialMaterial(crate::redact::Redacted<Vec<u8>>);

impl CredentialMaterial {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(crate::redact::Redacted::new(bytes.into()))
    }

    /// Exposes secret bytes to a trusted credential-store/provider adapter.
    pub fn expose_secret(&self) -> &[u8] {
        self.0.expose_secret().as_slice()
    }
}

impl fmt::Debug for CredentialMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::redact::opaque(formatter, "CredentialMaterial")
    }
}

/// Injected pacing seam for bounded read retries.
///
/// A read gateway never sleeps on its own: between bounded attempts it asks the
/// injected implementation to wait. Production injects a real blocking sleep;
/// deterministic tests inject a recording or zero-delay implementation, so no
/// test ever depends on ambient sleeping and no retry can become a hot loop.
pub trait RetryDelay {
    /// Pauses for `delay` before the next bounded attempt.
    fn wait(&mut self, delay: Duration);
}

/// The production pacing implementation: one bounded blocking sleep per wait.
///
/// A zero delay is a no-op, so a policy that computes no backoff never performs
/// a pointless sleep call.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadRetryDelay;

impl RetryDelay for ThreadRetryDelay {
    fn wait(&mut self, delay: Duration) {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }
}

/// Which transient read categories a bounded retry may repeat.
///
/// The scope is explicit at the call site because the safe set is not the same
/// for every read: a definition acquisition never feeds an observation result,
/// while a profile read can be the observation half of a mutation execution and
/// must not silently absorb an unavailable provider response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadRetryScope {
    /// Repeat only the explicit provider rate-limit refusal.
    RateLimitOnly,
    /// Repeat rate-limit and transient provider-unavailable failures.
    RateLimitAndTransient,
}

impl ReadRetryScope {
    /// The retry scope for one profile synchronization reason.
    ///
    /// Only a mutation observation ([`ProfileSyncReason::Preflight`] or
    /// [`ProfileSyncReason::PostAction`]) is restricted to the rate-limit-only
    /// scope, so a transient provider-unavailable response is reported instead
    /// of being absorbed by a repeat. Every other reason is a plain read that
    /// may repeat both the explicit rate limit and a transient unavailable
    /// failure under the bounded policy.
    pub const fn for_profile_reason(reason: ProfileSyncReason) -> Self {
        if reason.observes_mutation() {
            Self::RateLimitOnly
        } else {
            Self::RateLimitAndTransient
        }
    }
}

/// One bounded read-retry policy: a small attempt ceiling and bounded backoff.
///
/// `max_attempts` counts total attempts including the first, so
/// [`ReadRetryPolicy::single_attempt`] never repeats and every constructed value
/// is clamped to `1..=MAX_ATTEMPTS`. The backoff derives from `base_delay` and is
/// capped by `max_delay`, so neither the attempt count nor any wait is unbounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadRetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl ReadRetryPolicy {
    /// The hard ceiling on total attempts, independent of caller input.
    pub const MAX_ATTEMPTS: u32 = 5;

    /// Builds a policy, clamping attempts to `1..=MAX_ATTEMPTS` and ordering the
    /// backoff bounds so `base_delay <= max_delay`.
    pub const fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        let max_attempts = if max_attempts < 1 {
            1
        } else if max_attempts > Self::MAX_ATTEMPTS {
            Self::MAX_ATTEMPTS
        } else {
            max_attempts
        };
        let (base_delay, max_delay) = if base_delay.as_nanos() > max_delay.as_nanos() {
            (max_delay, base_delay)
        } else {
            (base_delay, max_delay)
        };
        Self {
            max_attempts,
            base_delay,
            max_delay,
        }
    }

    /// A policy that performs exactly one attempt and never repeats.
    pub const fn single_attempt() -> Self {
        Self {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    /// The total number of attempts, including the first.
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// The first backoff before a repeat.
    pub const fn base_delay(&self) -> Duration {
        self.base_delay
    }

    /// The inclusive ceiling on any single wait.
    pub const fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// The bounded exponential backoff to wait before `attempt + 1`.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if self.base_delay.is_zero() {
            return Duration::ZERO;
        }
        let shift = attempt.saturating_sub(1).min(16);
        let factor = 1u128 << shift;
        let nanos = self
            .base_delay
            .as_nanos()
            .saturating_mul(factor)
            .min(self.max_delay.as_nanos());
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

impl Default for ReadRetryPolicy {
    fn default() -> Self {
        Self::new(3, Duration::from_millis(200), Duration::from_secs(2))
    }
}

/// The provider-neutral classification of one HTTP `Retry-After` header.
///
/// A transport classifies the raw header value into this bounded shape before
/// it crosses into the gateway, so the original provider text is never retained
/// or rendered. The classification is clock-independent: an absolute date is
/// stored as whole seconds since the Unix epoch and is only resolved into a
/// concrete wait with an injected clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAfterHeader {
    /// No `Retry-After` header was observed; the bounded policy backoff applies.
    Absent,
    /// The header carried an integer number of seconds.
    DeltaSeconds(u64),
    /// The header carried an absolute HTTP date, as seconds since the Unix epoch.
    HttpDate(u64),
    /// The header was present but not a valid delta-seconds value or HTTP date.
    Invalid,
}

impl RetryAfterHeader {
    /// Resolves this header into a bounded pacing decision.
    ///
    /// The `now` closure is consulted only for an absolute-date header, and only
    /// because a date names an instant that must be compared against the
    /// injected clock. An unavailable clock makes a date header unusable rather
    /// than guessed; a date already in the past resolves to a zero wait. The
    /// returned wait is not itself clamped here: the bounded policy owns the
    /// ceiling and suppresses a repeat whose instruction exceeds it.
    pub fn resolve(self, now: impl FnOnce() -> Option<Timestamp>) -> RetryAfterHint {
        match self {
            Self::Absent => RetryAfterHint::Absent,
            Self::DeltaSeconds(seconds) => RetryAfterHint::Wait(Duration::from_secs(seconds)),
            Self::HttpDate(seconds) => match now() {
                Some(now) => {
                    let wait = seconds.saturating_sub(now.seconds());
                    RetryAfterHint::Wait(Duration::from_secs(wait))
                }
                None => RetryAfterHint::Unusable,
            },
            Self::Invalid => RetryAfterHint::Unusable,
        }
    }
}

/// A provider pacing instruction attached to one read failure.
///
/// This is the resolved, bounded decision the retry runtime acts on. `Absent`
/// leaves pacing entirely to the policy backoff; `Wait` honors the provider's
/// instruction only while it fits the policy ceiling; `Unusable` means the
/// instruction could not be honored (a malformed or uninterpretable header, or
/// an absolute date whose clock was unavailable), so the read must not repeat.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAfterHint {
    /// No usable provider instruction; the bounded policy backoff applies.
    Absent,
    /// The provider instructed a wait of this duration.
    Wait(Duration),
    /// The provider instruction cannot be honored; the read must not repeat.
    Unusable,
}

/// Parses one raw HTTP `Retry-After` header value into a bounded classification.
///
/// A non-empty run of ASCII digits is the integer delta-seconds form. An empty
/// value, an overflowing delta-seconds value, or any other token that is not a
/// valid HTTP date is [`RetryAfterHeader::Invalid`], never a guessed wait. Both
/// the canonical IMF-fixdate and the two obsolete HTTP date forms are accepted;
/// a two-digit RFC 850 year follows the documented 1970 cut-off convention.
pub fn parse_retry_after_header(value: &str) -> RetryAfterHeader {
    let value = value.trim();
    if value.is_empty() {
        return RetryAfterHeader::Invalid;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return match value.parse::<u64>() {
            Ok(seconds) => RetryAfterHeader::DeltaSeconds(seconds),
            Err(_) => RetryAfterHeader::Invalid,
        };
    }
    match parse_http_date(value) {
        Some(seconds) => RetryAfterHeader::HttpDate(seconds),
        None => RetryAfterHeader::Invalid,
    }
}

/// Parses one raw HTTP `Retry-After` header from its raw bytes.
///
/// This is the transport-facing entry point: a header is classified from the
/// exact bytes the provider sent, so the absent-header decision stays separate
/// from the present-but-unusable one. A byte sequence that is not valid UTF-8 is
/// [`RetryAfterHeader::Invalid`] -- a *present* but unusable instruction -- never
/// the same as an absent header, so a non-textual header suppresses a repeat
/// instead of silently falling back to the bounded policy backoff. A valid
/// UTF-8 sequence is classified by [`parse_retry_after_header`].
pub fn parse_retry_after_header_bytes(value: &[u8]) -> RetryAfterHeader {
    match std::str::from_utf8(value) {
        Ok(text) => parse_retry_after_header(text),
        Err(_) => RetryAfterHeader::Invalid,
    }
}

/// Parses one HTTP date, accepting the canonical and both obsolete forms.
///
/// Every accepted form names a weekday alongside a calendar date. A weekday that
/// contradicts the date is rejected: the header is internally inconsistent, so
/// it is not a usable instruction and must not be resolved into a wait.
fn parse_http_date(value: &str) -> Option<u64> {
    parse_imf_fixdate(value)
        .or_else(|| parse_rfc850(value))
        .or_else(|| parse_asctime(value))
}

/// `Sun, 06 Nov 1994 08:49:37 GMT`
fn parse_imf_fixdate(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let weekday = parts.next()?;
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    let time = parts.next()?;
    let zone = parts.next()?;
    if parts.next().is_some() || zone != "GMT" {
        return None;
    }
    let weekday = weekday_index(weekday.strip_suffix(',')?)?;
    let (hour, minute, second) = parse_time_of_day(time)?;
    unix_seconds(
        parse_year4(year)?,
        month_number(month)?,
        parse_day_of_month(day)?,
        hour,
        minute,
        second,
        weekday,
    )
}

/// `Sunday, 06-Nov-94 08:49:37 GMT`
fn parse_rfc850(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let weekday = parts.next()?;
    let date = parts.next()?;
    let time = parts.next()?;
    let zone = parts.next()?;
    if parts.next().is_some() || zone != "GMT" {
        return None;
    }
    let weekday = weekday_index(weekday.strip_suffix(',')?)?;
    let mut date_parts = date.split('-');
    let day = date_parts.next()?;
    let month = date_parts.next()?;
    let raw_year = date_parts.next()?;
    if date_parts.next().is_some() {
        return None;
    }
    let year = match raw_year.len() {
        2 => {
            let two = parse_two_digits(raw_year)?;
            // The HTTP convention maps a two-digit year below 70 into the 2000s
            // and 70..=99 into the 1900s.
            if two < 70 {
                2000 + i64::from(two)
            } else {
                1900 + i64::from(two)
            }
        }
        4 => parse_year4(raw_year)?,
        _ => return None,
    };
    let (hour, minute, second) = parse_time_of_day(time)?;
    unix_seconds(
        year,
        month_number(month)?,
        parse_day_of_month(day)?,
        hour,
        minute,
        second,
        weekday,
    )
}

/// `Sun Nov  6 08:49:37 1994`
fn parse_asctime(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let weekday = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    let time = parts.next()?;
    let year = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let weekday = weekday_index(weekday)?;
    let (hour, minute, second) = parse_time_of_day(time)?;
    unix_seconds(
        parse_year4(year)?,
        month_number(month)?,
        parse_day_of_month(day)?,
        hour,
        minute,
        second,
        weekday,
    )
}

/// Maps one weekday token to its index, `0` = Sunday through `6` = Saturday.
///
/// Both the three-letter spelling used by the canonical IMF-fixdate and the
/// `asctime` form and the full spelling used by RFC 850 are accepted. A trailing
/// comma is not part of the token and is stripped by the caller before this
/// lookup.
fn weekday_index(token: &str) -> Option<u32> {
    Some(match token {
        "Sun" | "Sunday" => 0,
        "Mon" | "Monday" => 1,
        "Tue" | "Tuesday" => 2,
        "Wed" | "Wednesday" => 3,
        "Thu" | "Thursday" => 4,
        "Fri" | "Friday" => 5,
        "Sat" | "Saturday" => 6,
        _ => return None,
    })
}

fn month_number(token: &str) -> Option<u32> {
    Some(match token {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// One or two ASCII digits naming a calendar day in `1..=31`.
fn parse_day_of_month(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > 2 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day: u32 = token.parse().ok()?;
    (1..=31).contains(&day).then_some(day)
}

/// Exactly two ASCII digits.
fn parse_two_digits(token: &str) -> Option<u32> {
    if token.len() != 2 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// Exactly four ASCII digits, bounded to a representable year at or after 1970.
fn parse_year4(token: &str) -> Option<i64> {
    if token.len() != 4 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i64 = token.parse().ok()?;
    (1970..=9999).contains(&year).then_some(year)
}

/// `HH:MM:SS` with two digits per field and an optional leap second.
fn parse_time_of_day(token: &str) -> Option<(u32, u32, u32)> {
    let mut parts = token.split(':');
    let hour = parse_two_digits(parts.next()?)?;
    let minute = parse_two_digits(parts.next()?)?;
    let second = parse_two_digits(parts.next()?)?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some((hour, minute, second))
}

/// Converts one validated UTC calendar instant into Unix-epoch seconds.
///
/// The day is range-checked against the month and the named weekday is checked
/// against the date before conversion, and a date before the epoch resolves to
/// zero rather than a negative value, so a stale instruction always yields a
/// zero wait.
fn unix_seconds(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    weekday: u32,
) -> Option<u64> {
    if day > days_in_month(year, month) || weekday_of(year, month, day) != weekday {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return Some(0);
    }
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    u64::try_from(seconds).ok()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Computes the weekday of one calendar date, `0` = Sunday through `6` = Saturday.
///
/// The Unix epoch, `1970-01-01`, was a Thursday, so the epoch day count is
/// offset by four. `days_from_civil` is exact for negative (pre-epoch) inputs, so
/// a stale pre-epoch date is compared correctly before it clamps to a zero wait.
fn weekday_of(year: i64, month: u32, day: u32) -> u32 {
    (days_from_civil(year, month, day) + 4).rem_euclid(7) as u32
}

/// Days between `1970-01-01` and `year-month-day` (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + i64::from(day_of_year);
    era * 146_097 + day_of_era - 719_468
}

/// One read attempt's sanitized failure plus an optional provider pacing hint.
///
/// The hint is the provider's own `Retry-After` signal when the adapter could
/// read it. It is non-secret, never rendered, and never escapes: it only shapes
/// the next bounded wait, always capped by the policy ceiling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadFailure {
    error: AppError,
    retry_after: RetryAfterHint,
}

impl ReadFailure {
    /// A failure that carries no provider pacing hint.
    pub const fn terminal(error: AppError) -> Self {
        Self {
            error,
            retry_after: RetryAfterHint::Absent,
        }
    }

    /// A classified failure that also carries a resolved provider pacing hint.
    pub const fn with_retry_after(error: AppError, retry_after: RetryAfterHint) -> Self {
        Self { error, retry_after }
    }

    /// The sanitized category-only failure.
    pub fn error(self) -> AppError {
        self.error
    }

    /// The resolved provider pacing hint, when one was observed.
    pub const fn retry_after(&self) -> RetryAfterHint {
        self.retry_after
    }
}

/// A bounded, paced read-retry runtime: policy plus injected delay.
///
/// It exists so a read adapter can repeat one idempotent request without ever
/// hot-looping, and so the same deterministic decision is testable without a
/// live clock, transport, or ambient sleep.
pub struct ReadRetry {
    policy: ReadRetryPolicy,
    delay: Box<dyn RetryDelay + Send + Sync>,
}

impl ReadRetry {
    /// Pairs one bounded policy with one injected pacing implementation.
    pub fn new(policy: ReadRetryPolicy, delay: impl RetryDelay + Send + Sync + 'static) -> Self {
        Self {
            policy,
            delay: Box::new(delay),
        }
    }

    /// The bounded policy this runtime applies.
    pub const fn policy(&self) -> ReadRetryPolicy {
        self.policy
    }

    /// A single-attempt runtime with no pacing: useful to keep a
    /// category-mapping test or a deliberately one-shot caller unchanged.
    pub fn single_attempt() -> Self {
        Self::new(ReadRetryPolicy::single_attempt(), ThreadRetryDelay)
    }

    /// Runs one read operation under the bounded policy.
    ///
    /// A repeat happens only while the failure category is in `scope` and the
    /// attempt ceiling has not been reached. Pacing is bounded in every case:
    /// an absent provider hint uses the policy backoff, a provider wait is
    /// honored as long as it fits the policy ceiling, and a provider wait that
    /// exceeds the ceiling, a malformed instruction, or an uninterpretable date
    /// suppresses the repeat entirely rather than silently waiting less than the
    /// provider instructed. The final failure is returned unchanged, including
    /// its category and sanitization.
    pub fn run<T, F>(&mut self, scope: ReadRetryScope, mut operation: F) -> Result<T, AppError>
    where
        F: FnMut() -> Result<T, ReadFailure>,
    {
        let mut attempt: u32 = 1;
        loop {
            match operation() {
                Ok(value) => return Ok(value),
                Err(failure) => {
                    let category = failure.error.category();
                    let retryable = match scope {
                        ReadRetryScope::RateLimitOnly => category.is_rate_limit(),
                        ReadRetryScope::RateLimitAndTransient => category.is_retryable_read(),
                    };
                    if !retryable || attempt >= self.policy.max_attempts() {
                        return Err(failure.error());
                    }
                    let backoff = self.policy.delay_for_attempt(attempt);
                    let wait = match failure.retry_after() {
                        RetryAfterHint::Unusable => return Err(failure.error()),
                        RetryAfterHint::Absent => backoff,
                        RetryAfterHint::Wait(provider) => {
                            if provider > self.policy.max_delay() {
                                return Err(failure.error());
                            }
                            provider.max(backoff)
                        }
                    };
                    self.delay.wait(wait);
                    attempt += 1;
                }
            }
        }
    }
}

impl Default for ReadRetry {
    fn default() -> Self {
        Self::new(ReadRetryPolicy::default(), ThreadRetryDelay)
    }
}

impl fmt::Debug for ReadRetry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadRetry")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// Stores one complete opaque access credential without prescribing persistence
/// or serialization.
///
/// The port carries the whole [`AccessCredential`] — the opaque material *and*
/// its expiry metadata — so a store can never silently discard the freshness
/// information an adapter needs to revalidate the credential later. A store
/// that cannot represent the expiry must fail closed instead of persisting the
/// bare material; there is no material-only overload.
pub trait CredentialStore {
    fn save(&mut self, credential: AccessCredential) -> Result<(), AppError>;
    fn load(&self) -> Result<Option<AccessCredential>, AppError>;
    fn delete(&mut self) -> Result<(), AppError>;
}

#[cfg(test)]
mod tests {
    use super::{
        DestinyMembership, ReadFailure, ReadRetry, ReadRetryPolicy, ReadRetryScope,
        RetryAfterHeader, RetryAfterHint, RetryDelay, ThreadRetryDelay, TransferGateway,
        TransferOutcome, TransferRejection, TransferRequest, parse_retry_after_header,
        parse_retry_after_header_bytes,
    };
    use crate::domain::ItemLocation;
    use crate::error::{AppError, ErrorCategory};
    use crate::ids::{CharacterId, ItemHash, ItemInstanceId, MembershipId, MembershipType};
    use crate::state::{ProfileSyncReason, Timestamp};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const NUMERIC_ID: &str = "4611686018484639825";

    #[test]
    fn membership_accepts_numeric_identifiers_and_exposes_typed_fields() {
        let membership =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new(NUMERIC_ID)).unwrap();
        assert_eq!(membership.membership_type(), MembershipType::new(3));
        assert_eq!(membership.membership_id().as_str(), NUMERIC_ID);
    }

    #[test]
    fn membership_rejects_empty_non_decimal_and_oversized_identifiers() {
        let oversized = "9".repeat(129);
        for identifier in ["", "abc", "123a", " 123", "١٢٣", oversized.as_str()] {
            assert_eq!(
                DestinyMembership::new(MembershipType::new(3), MembershipId::new(identifier)),
                Err(AppError::new(ErrorCategory::ProviderValidation)),
                "identifier {identifier:?} must be rejected"
            );
        }
        let boundary = "9".repeat(128);
        assert!(
            DestinyMembership::new(MembershipType::new(3), MembershipId::new(boundary)).is_ok(),
            "the durable identifier bound must be inclusive"
        );
    }

    #[test]
    fn membership_debug_redacts_the_identifier_but_keeps_the_type() {
        let membership =
            DestinyMembership::new(MembershipType::new(254), MembershipId::new(NUMERIC_ID))
                .unwrap();
        let rendered = format!("{membership:?}");
        assert!(rendered.contains("DestinyMembership"));
        assert!(rendered.contains("254"));
        assert!(
            !rendered.contains(NUMERIC_ID),
            "debug output leaked the membership identifier"
        );
    }

    #[test]
    fn membership_equality_covers_both_fields() {
        let base =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new(NUMERIC_ID)).unwrap();
        let same =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new(NUMERIC_ID)).unwrap();
        let other_type =
            DestinyMembership::new(MembershipType::new(2), MembershipId::new(NUMERIC_ID)).unwrap();
        let other_id =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new("7")).unwrap();

        assert_eq!(base, same);
        assert_ne!(base, other_type);
        assert_ne!(base, other_id);
    }

    /// One transfer request from the Vault to one exact character.
    fn membership() -> DestinyMembership {
        DestinyMembership::new(MembershipType::new(3), MembershipId::new(NUMERIC_ID))
            .expect("valid membership")
    }

    fn request(from: ItemLocation, to: ItemLocation) -> Result<TransferRequest, AppError> {
        TransferRequest::new(
            membership(),
            ItemInstanceId::new("instance-1"),
            ItemHash::new(1234),
            1,
            from,
            to,
        )
    }

    #[test]
    fn transfer_requests_accept_only_distinct_vault_or_character_endpoints() {
        let warlock = ItemLocation::Character(CharacterId::new("warlock"));

        let direct = request(ItemLocation::Vault, warlock.clone()).expect("valid request");
        assert!(!direct.to_vault());
        // The complete resolved membership travels with the request, so an
        // adapter needs no out-of-band account input.
        assert_eq!(direct.membership(), &membership());
        assert_eq!(
            direct.membership().membership_type(),
            MembershipType::new(3)
        );
        assert_eq!(direct.membership().membership_id().as_str(), NUMERIC_ID);
        assert_eq!(direct.item(), &ItemInstanceId::new("instance-1"));
        assert_eq!(direct.item_hash(), ItemHash::new(1234));
        assert_eq!(direct.quantity(), 1);
        assert_eq!(direct.from(), &ItemLocation::Vault);
        assert_eq!(direct.to(), &warlock);

        let into_vault = TransferRequest::new(
            membership(),
            ItemInstanceId::new("instance-1"),
            ItemHash::new(1234),
            7,
            ItemLocation::Character(CharacterId::new("warlock")),
            ItemLocation::Vault,
        )
        .expect("valid request");
        assert!(into_vault.to_vault());
        assert_eq!(into_vault.quantity(), 7);
        assert_eq!(into_vault.to(), &ItemLocation::Vault);

        // Only the Vault and one exact character are transfer endpoints.
        for (from, to) in [
            (ItemLocation::other("future-location"), ItemLocation::Vault),
            (ItemLocation::Vault, ItemLocation::other("future-location")),
        ] {
            assert_eq!(
                request(from.clone(), to.clone()),
                Err(AppError::new(ErrorCategory::UnsupportedOperation)),
                "{from:?} -> {to:?} must be rejected"
            );
        }

        // A transfer to where the item already is has no provider meaning.
        let character = ItemLocation::Character(CharacterId::new("warlock"));
        assert_eq!(
            request(ItemLocation::Vault, ItemLocation::Vault),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
        assert_eq!(
            request(character.clone(), character),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
    }

    #[test]
    fn transfer_request_debug_redacts_every_identifier_value() {
        let rendered = format!(
            "{:?}",
            request(
                ItemLocation::Vault,
                ItemLocation::Character(CharacterId::new("warlock"))
            )
            .expect("valid request")
        );
        assert!(rendered.contains("TransferRequest"));
        // Identifiers redact their values in debug output: the opaque ids print
        // only their type name, and the resolved membership keeps its non-secret
        // provider type while redacting the account identifier.
        assert!(rendered.contains("ItemInstanceId"));
        assert!(rendered.contains("MembershipType"));
        for secret in ["instance-1", NUMERIC_ID, "warlock"] {
            assert!(
                !rendered.contains(secret),
                "transfer request debug leaked {secret}"
            );
        }
    }

    #[test]
    fn transfer_outcomes_classify_applied_refused_transient_and_uncertain() {
        assert!(TransferOutcome::Transferred.applied());
        assert_eq!(TransferOutcome::Transferred.rejection(), None);
        assert!(!TransferOutcome::Transient.applied());
        assert_eq!(TransferOutcome::Transient.rejection(), None);
        assert!(!TransferOutcome::Uncertain.applied());
        assert_eq!(TransferOutcome::Uncertain.rejection(), None);

        for (rejection, category) in [
            (
                TransferRejection::NotTransferable,
                ErrorCategory::UnsupportedOperation,
            ),
            (
                TransferRejection::Equipped,
                ErrorCategory::UnsupportedOperation,
            ),
            (
                TransferRejection::DestinationFull,
                ErrorCategory::ProviderValidation,
            ),
            (
                TransferRejection::InvalidRequest,
                ErrorCategory::ProviderValidation,
            ),
        ] {
            let outcome = TransferOutcome::Rejected(rejection);
            assert!(!outcome.applied());
            assert_eq!(outcome.rejection(), Some(rejection));
            assert_eq!(rejection.category(), category);
        }
    }

    #[test]
    fn a_transfer_gateway_issues_one_request_per_step_without_retrying() {
        #[derive(Default)]
        struct Recording {
            items: Vec<String>,
        }

        impl TransferGateway for Recording {
            fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError> {
                self.items.push(request.item().as_str().to_owned());
                Ok(TransferOutcome::Transient)
            }
        }

        let mut gateway = Recording::default();
        let transfer = request(
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("warlock")),
        )
        .expect("valid request");

        assert_eq!(gateway.transfer(&transfer), Ok(TransferOutcome::Transient));
        // One call is one step: the gateway never retries on its own.
        assert_eq!(gateway.items, ["instance-1"]);
    }

    /// Records the exact paced waits a bounded retry performed, with no sleep.
    #[derive(Clone, Default)]
    struct RecordingDelay {
        waits: Arc<Mutex<Vec<Duration>>>,
    }

    impl RecordingDelay {
        fn waits(&self) -> Vec<Duration> {
            self.waits.lock().expect("delay log lock").clone()
        }
    }

    impl RetryDelay for RecordingDelay {
        fn wait(&mut self, delay: Duration) {
            self.waits.lock().expect("delay log lock").push(delay);
        }
    }

    fn rate_limit() -> AppError {
        AppError::new(ErrorCategory::ProviderRateLimit)
    }

    #[test]
    fn read_retry_policy_clamps_attempts_and_orders_backoff_bounds() {
        assert_eq!(ReadRetryPolicy::single_attempt().max_attempts(), 1);
        assert_eq!(
            ReadRetryPolicy::new(0, Duration::ZERO, Duration::ZERO).max_attempts(),
            1
        );
        assert_eq!(
            ReadRetryPolicy::new(99, Duration::ZERO, Duration::ZERO).max_attempts(),
            ReadRetryPolicy::MAX_ATTEMPTS
        );
        // A reversed pair is ordered so the cap is never below the base.
        let ordered = ReadRetryPolicy::new(2, Duration::from_secs(5), Duration::from_secs(1));
        assert_eq!(ordered.base_delay(), Duration::from_secs(1));
        assert_eq!(ordered.max_delay(), Duration::from_secs(5));
    }

    #[test]
    fn bounded_read_backoff_is_exponential_and_capped() {
        let policy =
            ReadRetryPolicy::new(5, Duration::from_millis(100), Duration::from_millis(250));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(200));
        // The third would be 400 ms but the cap holds it at 250 ms.
        assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(250));
        assert_eq!(
            policy.delay_for_attempt(u32::MAX),
            Duration::from_millis(250)
        );
        assert_eq!(
            ReadRetryPolicy::single_attempt().delay_for_attempt(1),
            Duration::ZERO
        );
    }

    #[test]
    fn bounded_read_retry_repeats_a_rate_limit_to_success() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let policy = ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_millis(100));
        let mut retry = ReadRetry::new(policy, delay);
        let mut attempts = 0;

        let value = retry
            .run(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                if attempts < 3 {
                    Err(ReadFailure::terminal(rate_limit()))
                } else {
                    Ok(7)
                }
            })
            .expect("third attempt succeeds");

        assert_eq!(value, 7);
        assert_eq!(attempts, 3);
        // Exactly one wait per repeat, paced by the injected delay: no ambient
        // sleep and no hot loop.
        assert_eq!(
            waits.waits(),
            vec![Duration::from_millis(10), Duration::from_millis(20)]
        );
    }

    #[test]
    fn bounded_read_retry_exhausts_at_the_attempt_ceiling() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let policy = ReadRetryPolicy::new(3, Duration::from_millis(5), Duration::from_millis(20));
        let mut retry = ReadRetry::new(policy, delay);
        let mut attempts = 0;

        let error = retry
            .run::<(), _>(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                Err(ReadFailure::terminal(rate_limit()))
            })
            .expect_err("exhausted retry fails");

        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(attempts, 3);
        // Two repeats for three attempts, never a third wait.
        assert_eq!(waits.waits().len(), 2);
    }

    #[test]
    fn bounded_read_retry_never_repeats_a_non_transient_failure() {
        for category in [
            ErrorCategory::ProviderValidation,
            ErrorCategory::ProviderAuthentication,
            ErrorCategory::VerificationFailed,
            ErrorCategory::AmbiguousMatch,
        ] {
            let delay = RecordingDelay::default();
            let waits = delay.clone();
            let mut retry = ReadRetry::new(ReadRetryPolicy::default(), delay);
            let mut attempts = 0;

            let error = retry
                .run::<(), _>(ReadRetryScope::RateLimitAndTransient, || {
                    attempts += 1;
                    Err(ReadFailure::terminal(AppError::new(category)))
                })
                .expect_err("non-transient failure is not retried");

            assert_eq!(error.category(), category);
            assert_eq!(attempts, 1, "{category:?} must not be repeated");
            assert!(waits.waits().is_empty());
        }
    }

    #[test]
    fn rate_limit_scope_never_repeats_a_provider_unavailable_failure() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let mut retry = ReadRetry::new(ReadRetryPolicy::default(), delay);
        let mut attempts = 0;

        let error = retry
            .run::<(), _>(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                Err(ReadFailure::terminal(AppError::new(
                    ErrorCategory::ProviderUnavailable,
                )))
            })
            .expect_err("unavailable is outside the rate-limit scope");

        assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
        assert_eq!(attempts, 1);
        assert!(waits.waits().is_empty());
    }

    #[test]
    fn transient_scope_repeats_a_provider_unavailable_failure() {
        let delay = RecordingDelay::default();
        let mut retry = ReadRetry::new(
            ReadRetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(4)),
            delay,
        );
        let mut attempts = 0;

        let value = retry
            .run(ReadRetryScope::RateLimitAndTransient, || {
                attempts += 1;
                if attempts == 1 {
                    Err(ReadFailure::terminal(AppError::new(
                        ErrorCategory::ProviderUnavailable,
                    )))
                } else {
                    Ok(1)
                }
            })
            .expect("second attempt succeeds");

        assert_eq!(value, 1);
        assert_eq!(attempts, 2);
    }

    #[test]
    fn bounded_read_retry_honors_a_retry_after_within_the_policy_ceiling() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let policy = ReadRetryPolicy::new(2, Duration::from_millis(10), Duration::from_secs(30));
        let mut retry = ReadRetry::new(policy, delay);
        let mut attempts = 0;

        retry
            .run(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                if attempts == 1 {
                    Err(ReadFailure::with_retry_after(
                        rate_limit(),
                        RetryAfterHint::Wait(Duration::from_secs(5)),
                    ))
                } else {
                    Ok(())
                }
            })
            .expect("hint-paced retry succeeds");

        // The provider hint wins over the smaller 10 ms backoff.
        assert_eq!(attempts, 2);
        assert_eq!(waits.waits(), vec![Duration::from_secs(5)]);
    }

    #[test]
    fn bounded_read_retry_does_not_shorten_a_retry_after_beyond_the_ceiling() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let policy = ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(30));
        let mut retry = ReadRetry::new(policy, delay);
        let mut attempts = 0;

        let error = retry
            .run::<(), _>(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                Err(ReadFailure::with_retry_after(
                    rate_limit(),
                    RetryAfterHint::Wait(Duration::from_secs(3_600)),
                ))
            })
            .expect_err("a wait beyond the ceiling suppresses the repeat");

        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(
            attempts, 1,
            "the read must not repeat sooner than instructed"
        );
        assert!(waits.waits().is_empty());
    }

    #[test]
    fn an_unusable_retry_after_hint_suppresses_the_repeat() {
        let delay = RecordingDelay::default();
        let waits = delay.clone();
        let mut retry = ReadRetry::new(ReadRetryPolicy::default(), delay);
        let mut attempts = 0;

        let error = retry
            .run::<(), _>(ReadRetryScope::RateLimitOnly, || {
                attempts += 1;
                Err(ReadFailure::with_retry_after(
                    rate_limit(),
                    RetryAfterHint::Unusable,
                ))
            })
            .expect_err("an unusable instruction is terminal for this attempt");

        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(attempts, 1);
        assert!(waits.waits().is_empty());
    }

    #[test]
    fn read_failure_terminal_carries_no_pacing_hint() {
        let failure = ReadFailure::terminal(rate_limit());
        assert_eq!(
            failure.clone().error().category(),
            ErrorCategory::ProviderRateLimit
        );
        assert_eq!(failure.retry_after(), RetryAfterHint::Absent);
    }

    #[test]
    fn profile_sync_reason_selects_the_read_retry_scope() {
        for reason in [ProfileSyncReason::Ordinary, ProfileSyncReason::Forced] {
            assert!(!reason.observes_mutation(), "{reason:?} is a plain read");
            assert_eq!(
                ReadRetryScope::for_profile_reason(reason),
                ReadRetryScope::RateLimitAndTransient,
                "{reason:?} may repeat a transient failure"
            );
        }
        for reason in [ProfileSyncReason::Preflight, ProfileSyncReason::PostAction] {
            assert!(reason.observes_mutation(), "{reason:?} observes a mutation");
            assert_eq!(
                ReadRetryScope::for_profile_reason(reason),
                ReadRetryScope::RateLimitOnly,
                "{reason:?} must report an unavailable observation"
            );
        }
    }

    #[test]
    fn retry_after_header_classifies_delta_seconds_dates_and_invalid_values() {
        assert_eq!(
            parse_retry_after_header("0"),
            RetryAfterHeader::DeltaSeconds(0)
        );
        assert_eq!(
            parse_retry_after_header(" 120 "),
            RetryAfterHeader::DeltaSeconds(120)
        );
        // The canonical IMF-fixdate and both obsolete forms name one instant.
        let imf = parse_retry_after_header("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(imf, RetryAfterHeader::HttpDate(784_111_777));
        assert_eq!(
            imf,
            parse_retry_after_header("Sunday, 06-Nov-94 08:49:37 GMT")
        );
        assert_eq!(imf, parse_retry_after_header("Sun Nov  6 08:49:37 1994"));
        assert_eq!(
            parse_retry_after_header("Wed, 21 Oct 2015 07:28:00 GMT"),
            RetryAfterHeader::HttpDate(1_445_412_480)
        );
        for invalid in [
            "",
            " ",
            "-1",
            "1.5",
            "not-a-date",
            "Wed, 32 Oct 2015 07:28:00 GMT",
            "Wed, 21 Xxx 2015 07:28:00 GMT",
            "Wed, 21 Oct 2015 25:28:00 GMT",
            "Wed, 21 Oct 2015 07:28:00 UTC",
            "99999999999999999999999",
        ] {
            assert_eq!(
                parse_retry_after_header(invalid),
                RetryAfterHeader::Invalid,
                "{invalid:?} must be invalid"
            );
        }
    }

    #[test]
    fn retry_after_header_bytes_reject_non_utf8_as_invalid() {
        // A present header is classified from its raw bytes, so a non-UTF-8
        // value is Invalid -- never Absent -- and therefore suppresses a retry.
        assert_eq!(
            parse_retry_after_header_bytes(b"120"),
            RetryAfterHeader::DeltaSeconds(120)
        );
        assert_eq!(
            parse_retry_after_header_bytes(b"Sun, 06 Nov 1994 08:49:37 GMT"),
            RetryAfterHeader::HttpDate(784_111_777)
        );
        let invalid = parse_retry_after_header_bytes([0xff, 0xfe].as_slice());
        assert_eq!(invalid, RetryAfterHeader::Invalid);
        assert_eq!(
            invalid.resolve(|| panic!("clock must not be read")),
            RetryAfterHint::Unusable
        );
        // A valid UTF-8 value outside visible ASCII is still unusable rather
        // than silently absent.
        assert_eq!(
            parse_retry_after_header_bytes("é".as_bytes()),
            RetryAfterHeader::Invalid
        );
    }

    #[test]
    fn retry_after_header_rejects_a_weekday_mismatching_the_date() {
        // 2015-10-21 was a Wednesday. A header naming a different weekday
        // contradicts its own date and must not resolve into a wait.
        for value in [
            "Mon, 21 Oct 2015 07:28:00 GMT",
            "Tue, 21 Oct 2015 07:28:00 GMT",
            "Thursday, 21-Oct-15 07:28:00 GMT",
            "Fri Oct 21 07:28:00 2015",
        ] {
            let header = parse_retry_after_header(value);
            assert_eq!(
                header,
                RetryAfterHeader::Invalid,
                "{value:?} must be rejected as a mismatched weekday"
            );
            assert_eq!(
                header.resolve(|| panic!("clock must not be read")),
                RetryAfterHint::Unusable,
                "{value:?} must suppress a retry"
            );
        }
        // The matching weekday still resolves the identical instant.
        assert_eq!(
            parse_retry_after_header("Wed, 21 Oct 2015 07:28:00 GMT"),
            RetryAfterHeader::HttpDate(1_445_412_480)
        );
        assert_eq!(
            parse_retry_after_header("Wednesday, 21-Oct-15 07:28:00 GMT"),
            RetryAfterHeader::HttpDate(1_445_412_480)
        );
        assert_eq!(
            parse_retry_after_header("Wed Oct 21 07:28:00 2015"),
            RetryAfterHeader::HttpDate(1_445_412_480)
        );
    }

    #[test]
    fn retry_after_header_resolves_dates_with_the_injected_clock() {
        let header = RetryAfterHeader::HttpDate(1_000);
        // A future date becomes its exact remaining wait.
        assert_eq!(
            header.resolve(|| Some(Timestamp::new(940))),
            RetryAfterHint::Wait(Duration::from_secs(60))
        );
        // A past date becomes a zero wait, never a negative one.
        assert_eq!(
            header.resolve(|| Some(Timestamp::new(1_500))),
            RetryAfterHint::Wait(Duration::ZERO)
        );
        // An unavailable clock makes an absolute date unusable.
        assert_eq!(header.resolve(|| None), RetryAfterHint::Unusable);

        // Delta-seconds and invalid values never consult the clock.
        assert_eq!(
            RetryAfterHeader::DeltaSeconds(7).resolve(|| panic!("clock must not be read")),
            RetryAfterHint::Wait(Duration::from_secs(7))
        );
        assert_eq!(
            RetryAfterHeader::Invalid.resolve(|| panic!("clock must not be read")),
            RetryAfterHint::Unusable
        );
        assert_eq!(
            RetryAfterHeader::Absent.resolve(|| panic!("clock must not be read")),
            RetryAfterHint::Absent
        );
    }

    #[test]
    fn thread_retry_delay_is_a_noop_for_a_zero_delay() {
        // The production implementation must not sleep for a zero wait; this
        // call would hang the suite if it did a fixed sleep instead.
        let mut delay = ThreadRetryDelay;
        delay.wait(Duration::ZERO);
    }
}
