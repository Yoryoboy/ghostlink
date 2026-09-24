//! Minimal provider-neutral behavioral ports.

use crate::auth::{AccessCredential, AuthorizationCallback, AuthorizationCode, AuthorizationState};
use crate::domain::{
    Character, InventorySnapshot, ItemDefinition, ItemInstance, ItemLocation, SyncMetadata,
};
use crate::error::{AppError, ErrorCategory};
use crate::ids::{
    BucketHash, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId, MembershipType, PlanId,
};
use crate::query::{DefinitionSearchQuery, InstanceSearchQuery};
use crate::state::{Resolution, SourceMarker, Timestamp};
use std::collections::HashSet;
use std::fmt;

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

/// Exchanges a code for public-client access material; no refresh or secret API exists here.
pub trait PublicTokenExchange {
    fn exchange(&mut self, code: AuthorizationCode) -> Result<AccessCredential, AppError>;
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
    /// permutation of the *items* themselves is not.
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

        let mut left_items: Vec<&ItemInstance> = self.items.iter().collect();
        let mut right_items: Vec<&ItemInstance> = other.items.iter().collect();
        left_items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        right_items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        left_items == right_items
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
    /// the durable repository's own storage constraints.
    pub fn is_structurally_valid(&self) -> bool {
        let mut character_ids = HashSet::with_capacity(self.characters.len());
        for character in &self.characters {
            if !character_ids.insert(character.id.clone()) {
                return false;
            }
        }

        let mut instance_ids = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            if !instance_ids.insert(item.instance_id.clone()) {
                return false;
            }
        }

        for item in &self.items {
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDefinitionCandidate {
    pub hash: ItemHash,
    pub bucket_hash: Option<BucketHash>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    pub element: Option<String>,
    pub tags: Vec<String>,
    pub version: DefinitionVersion,
}

/// One versioned batch of untrusted definition candidates from a Manifest gateway.
///
/// Neither the batch version nor its candidates are trusted: they may be blank,
/// oversized, inconsistent, duplicated, or incomplete. The application Manifest
/// service validates and normalizes the batch before any repository mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestAcquisition {
    pub version: DefinitionVersion,
    pub definitions: Vec<ManifestDefinitionCandidate>,
}

/// Returns untrusted normalized-shape candidates, not locations, URLs, or
/// serialized payloads.
pub trait ManifestGateway {
    fn load_definitions(&mut self) -> Result<ManifestAcquisition, AppError>;
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
/// The four variants are the only terminal shapes a transfer adapter may report
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
            Self::Rejected(_) | Self::Transient | Self::Uncertain => false,
        }
    }

    /// The bounded refusal, when the provider deterministically refused the step.
    pub const fn rejection(self) -> Option<TransferRejection> {
        match self {
            Self::Rejected(rejection) => Some(rejection),
            Self::Transferred | Self::Transient | Self::Uncertain => None,
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
/// - `Err(AppError)` is reserved for a failure in which no provider mutation can
///   have occurred, such as missing or rejected credentials, missing runtime
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
        DestinyMembership, TransferGateway, TransferOutcome, TransferRejection, TransferRequest,
    };
    use crate::domain::ItemLocation;
    use crate::error::{AppError, ErrorCategory};
    use crate::ids::{CharacterId, ItemHash, ItemInstanceId, MembershipId, MembershipType};

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
}
