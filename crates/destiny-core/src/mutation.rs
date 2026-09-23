//! Pure, provider-neutral mutation planning and safety policy.
//!
//! # Scope
//!
//! This module owns the *contracts* shared by every future `move_item`
//! adapter and the application planner that composes them. It is deliberately
//! I/O-free: no function here reads a repository, calls a provider, acquires a
//! credential, or transfers an item. Planning reads the synchronized
//! projection, describes expected effects, and binds one concrete item and
//! destination to a short-lived revision-scoped plan; remote execution belongs
//! to a later work unit.
//!
//! # Semantic selectors
//!
//! A move request is expressed with two validated selectors:
//!
//! - [`ItemSelector`] is either one exact [`ItemInstanceId`] or a bounded,
//!   normalized [`SearchTerm`] resolved through the existing definition search.
//!   Exact instance selection is never re-interpreted as a name, and a semantic
//!   term never silently selects an instance from an ambiguous match.
//! - [`DestinationSelector`] is the Vault, one exact [`CharacterId`], or a
//!   bounded label matched case-insensitively against the normalized character
//!   class or name.
//!
//! Both semantic forms reuse [`SearchTerm`], so the single normalization and
//! bound rule from the query contract applies here too.
//!
//! # Explicit intent
//!
//! [`MoveRequest`] carries a [`MoveIntentAuthorization`] fact rather than an
//! approval artifact. Explicit user intent to move one identified item is the
//! whole authorization for the direct single-call adapter path; absent or
//! ambiguous intent is rejected by [`MoveIntentAuthorization::authorize`]. No
//! preview token, confirmation value, approval ceremony, or second call exists
//! in this contract.
//!
//! # Deterministic steps and effects
//!
//! [`plan_steps`] derives the ordered transfer steps for a source/destination
//! pair: a single direct step when either endpoint is the Vault, and exactly two
//! steps through the Vault for a character-to-character move. The list is bounded
//! by [`MAX_MOVE_STEPS`]. [`MovePlan::effects`] reports the sanitized
//! postcondition, and neither record contains a credential, a raw provider
//! payload, or an adapter-specific value.
//!
//! # Bounded lifetime and binding
//!
//! [`PlanLifetime`] is the only caller-configured lifetime policy, and it is
//! bounded by [`MAX_PLAN_LIFETIME_SECONDS`]; a timestamp overflow fails with the
//! existing [`ErrorCategory::ClockOverflow`] error. [`MovePlan::binding`]
//! re-checks expiry, membership, revision, current source, resolved item, and
//! resolved destination against a preflight snapshot and reports a specific
//! [`PlanBinding`] outcome instead of guessing.

use crate::{
    domain::{Character, ItemLocation},
    error::{AppError, ErrorCategory},
    ids::{CharacterId, ItemInstanceId, MembershipId, PlanId, SyncRevision},
    query::{SearchTerm, normalize_search_text},
    state::{Resolution, Timestamp},
};

/// Maximum number of ordered steps one move plan may contain.
///
/// A direct move needs one step; a character-to-character move needs two so the
/// Vault is used exactly once as an intermediary. No plan may exceed this, which
/// keeps the result finite and reviewable.
pub const MAX_MOVE_STEPS: usize = 2;

/// Maximum accepted plan lifetime in seconds.
///
/// A plan is a short-lived safety and state-binding object, never an approval
/// artifact, so its expiry is bounded. A caller supplies the concrete duration
/// through [`PlanLifetime`]; this constant is the upper bound every plan is
/// checked against.
pub const MAX_PLAN_LIFETIME_SECONDS: u64 = 300;

/// A validated, semantics-preserving item selector.
///
/// An exact variant matches one concrete instance by identity. A semantic
/// variant is a validated, normalized definition term; the planner resolves it
/// through definition and instance search and returns ambiguity rather than
/// ranking candidates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemSelector {
    /// One exact concrete instance.
    Instance(ItemInstanceId),
    /// A bounded, normalized definition term.
    Semantic(SearchTerm),
}

impl ItemSelector {
    /// Selects one exact concrete instance.
    pub fn instance(instance_id: ItemInstanceId) -> Self {
        Self::Instance(instance_id)
    }

    /// Validates and normalizes one semantic definition term.
    ///
    /// An empty, oversized, or control-bearing term is rejected with the
    /// sanitized [`ErrorCategory::ProviderValidation`] category; the rejected
    /// value never appears in the error.
    pub fn semantic(text: &str) -> Result<Self, AppError> {
        SearchTerm::new(text).map(Self::Semantic)
    }

    /// The exact instance identifier, when this selector is exact.
    pub fn instance_id(&self) -> Option<&ItemInstanceId> {
        match self {
            Self::Instance(instance_id) => Some(instance_id),
            Self::Semantic(_) => None,
        }
    }

    /// The normalized semantic term, when this selector is semantic.
    pub fn semantic_term(&self) -> Option<&SearchTerm> {
        match self {
            Self::Semantic(term) => Some(term),
            Self::Instance(_) => None,
        }
    }

    /// True only for exact instance selection.
    pub fn is_exact(&self) -> bool {
        matches!(self, Self::Instance(_))
    }
}

/// A validated destination selector.
///
/// The Vault is explicit, never a magic character name. An exact character
/// identifier is matched structurally; a label is matched case-insensitively
/// against the normalized character class or name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DestinationSelector {
    /// The explicit Vault destination.
    Vault,
    /// One exact character identifier.
    Character(CharacterId),
    /// One bounded, normalized character class or name label.
    CharacterLabel(SearchTerm),
}

impl DestinationSelector {
    /// The explicit Vault destination.
    pub fn vault() -> Self {
        Self::Vault
    }

    /// One exact character destination.
    pub fn character(id: CharacterId) -> Self {
        Self::Character(id)
    }

    /// Validates and normalizes one character class or name label.
    ///
    /// The same validation and normalization rule as a search term applies, so
    /// an empty, oversized, or control-bearing label is rejected.
    pub fn character_label(text: &str) -> Result<Self, AppError> {
        SearchTerm::new(text).map(Self::CharacterLabel)
    }

    /// True only for the explicit Vault destination.
    pub fn is_vault(&self) -> bool {
        matches!(self, Self::Vault)
    }
}

/// True when one character matches one destination selector.
///
/// Vault matches no character. An exact identifier matches on identity; a label
/// matches on normalized equality against the character's class or name, never a
/// substring, so a label can never accidentally match an unrelated character.
pub fn destination_matches(selector: &DestinationSelector, character: &Character) -> bool {
    match selector {
        DestinationSelector::Vault => false,
        DestinationSelector::Character(id) => &character.id == id,
        DestinationSelector::CharacterLabel(term) => {
            let class_matches = character
                .class
                .as_deref()
                .is_some_and(|class| normalize_search_text(class) == term.as_str());
            let name_matches = character
                .name
                .as_deref()
                .is_some_and(|name| normalize_search_text(name) == term.as_str());
            class_matches || name_matches
        }
    }
}

/// Resolves one destination selector against the current characters.
///
/// The Vault is always a unique destination. A character selector resolves to
/// [`Resolution::Unique`] only when exactly one character matches, to
/// [`Resolution::Missing`] when none does, and to [`Resolution::Ambiguous`] when
/// more than one does. Ambiguity is returned, never ranked.
pub fn resolve_destination(
    selector: &DestinationSelector,
    characters: &[Character],
) -> Resolution<ItemLocation> {
    if selector.is_vault() {
        return Resolution::Unique(ItemLocation::Vault);
    }
    let mut matched: Vec<ItemLocation> = characters
        .iter()
        .filter(|character| destination_matches(selector, character))
        .map(|character| ItemLocation::Character(character.id.clone()))
        .collect();
    if matched.is_empty() {
        return Resolution::Missing;
    }
    if matched.len() == 1 {
        return Resolution::Unique(matched.pop().expect("one destination match"));
    }
    Resolution::Ambiguous(matched)
}

/// Whether the caller stated explicit intent to move one identified item.
///
/// This is the shared authorization fact for the direct single-call adapter
/// path. It is deliberately three-valued so absent or ambiguous input can never
/// be silently upgraded to authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveIntentAuthorization {
    /// The caller explicitly stated intent to move one identified item.
    Explicit,
    /// No intent accompanied the request.
    Absent,
    /// The request did not attribute intent to exactly one move.
    Ambiguous,
}

/// The evaluated authorization outcome of one explicit-intent fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentAuthorizationOutcome {
    /// Explicit intent authorizes the move.
    Authorized,
    /// No explicit intent was supplied.
    MissingIntent,
    /// Intent was not attributable to exactly one move.
    AmbiguousIntent,
}

impl MoveIntentAuthorization {
    /// True only for explicitly stated intent.
    pub fn is_explicit(self) -> bool {
        matches!(self, Self::Explicit)
    }

    /// Evaluates this fact into a terminal authorization outcome.
    pub fn authorize(self) -> IntentAuthorizationOutcome {
        match self {
            Self::Explicit => IntentAuthorizationOutcome::Authorized,
            Self::Absent => IntentAuthorizationOutcome::MissingIntent,
            Self::Ambiguous => IntentAuthorizationOutcome::AmbiguousIntent,
        }
    }
}

/// A provider-neutral semantic move request before resolution or planning.
///
/// It carries the validated selectors and the explicit-intent fact only. It
/// contains no preview token, confirmation value, approval artifact, credential,
/// or adapter-specific value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveRequest {
    pub membership: MembershipId,
    pub item: ItemSelector,
    pub destination: DestinationSelector,
    pub intent: MoveIntentAuthorization,
}

impl MoveRequest {
    /// Evaluates the carried intent fact into its authorization outcome.
    pub fn authorization(&self) -> IntentAuthorizationOutcome {
        self.intent.authorize()
    }
}

/// One ordered transfer step in a move plan.
///
/// The step carries a 1-based `ordinal` so the order is explicit, and the
/// source and destination locations for that step. Steps are derived
/// deterministically by [`plan_steps`]; the plan's concrete item is shared across
/// every step and therefore not repeated here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveStep {
    pub ordinal: u8,
    pub from: ItemLocation,
    pub to: ItemLocation,
}

impl MoveStep {
    /// Builds one ordered step.
    pub fn new(ordinal: u8, from: ItemLocation, to: ItemLocation) -> Self {
        Self { ordinal, from, to }
    }
}

/// The deterministic ordered steps for one source/destination pair.
///
/// A direct move (one endpoint is the Vault) produces exactly one step. A
/// character-to-character move produces exactly two steps: `source -> Vault`
/// followed by `Vault -> destination`, so the Vault is used exactly once as an
/// intermediary. A no-op or unsupported location is rejected with
/// [`MovePlanValidation`].
pub fn plan_steps(
    source: &ItemLocation,
    destination: &ItemLocation,
) -> Result<Vec<MoveStep>, MovePlanValidation> {
    if !supported_location(source) || !supported_location(destination) {
        return Err(MovePlanValidation::UnsupportedLocation);
    }
    if source == destination {
        return Err(MovePlanValidation::SameLocation);
    }
    let steps =
        if matches!(source, ItemLocation::Vault) || matches!(destination, ItemLocation::Vault) {
            vec![MoveStep::new(1, source.clone(), destination.clone())]
        } else {
            vec![
                MoveStep::new(1, source.clone(), ItemLocation::Vault),
                MoveStep::new(2, ItemLocation::Vault, destination.clone()),
            ]
        };
    debug_assert!(steps.len() <= MAX_MOVE_STEPS);
    Ok(steps)
}

/// The sanitized expected postcondition of one move plan.
///
/// It records only what a successful execution must make observable: the
/// concrete item leaves its source and arrives at its destination, using the
/// Vault exactly once when `via_vault` is true. It contains no credential, raw
/// provider payload, or adapter-specific value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveEffects {
    pub item: ItemInstanceId,
    pub source: ItemLocation,
    pub destination: ItemLocation,
    pub via_vault: bool,
}

impl MoveEffects {
    /// True when the expected outcome routes through the Vault.
    pub fn is_vault_mediated(&self) -> bool {
        self.via_vault
    }
}

/// A validated short-lived plan binding one item move to one revision.
///
/// The plan records the membership, concrete item, source, destination,
/// revision, creation and expiry timestamps, the ordered steps, and the expected
/// effects. It is an internal safety and state-binding object, never an approval
/// artifact, and it causes no remote mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MovePlan {
    pub plan_id: PlanId,
    pub membership: MembershipId,
    pub item: ItemInstanceId,
    pub source: ItemLocation,
    pub destination: ItemLocation,
    pub revision: SyncRevision,
    created_at: Timestamp,
    expires_at: Timestamp,
    steps: Vec<MoveStep>,
    effects: MoveEffects,
}

/// Compatibility alias retained for callers that used the original name.
pub type MoveIntent = MovePlan;

/// Why a move plan could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovePlanValidation {
    /// `created_at` is not strictly before `expires_at`.
    InvalidTimestamps,
    /// `expires_at - created_at` exceeds [`MAX_PLAN_LIFETIME_SECONDS`].
    LifetimeExceeded,
    /// The source and destination are the same location.
    SameLocation,
    /// A source or destination outside Vault/Character is unsupported.
    UnsupportedLocation,
}

impl MovePlan {
    /// Builds one validated plan, deriving its ordered steps and expected effects.
    ///
    /// The timestamps must define a strictly positive window no longer than
    /// [`MAX_PLAN_LIFETIME_SECONDS`], the endpoints must be supported and
    /// distinct, and the derived step list is bounded by [`MAX_MOVE_STEPS`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan_id: PlanId,
        membership: MembershipId,
        item: ItemInstanceId,
        source: ItemLocation,
        destination: ItemLocation,
        revision: SyncRevision,
        created_at: Timestamp,
        expires_at: Timestamp,
    ) -> Result<Self, MovePlanValidation> {
        if created_at >= expires_at {
            return Err(MovePlanValidation::InvalidTimestamps);
        }
        if expires_at.seconds() - created_at.seconds() > MAX_PLAN_LIFETIME_SECONDS {
            return Err(MovePlanValidation::LifetimeExceeded);
        }
        let steps = plan_steps(&source, &destination)?;
        let effects = MoveEffects {
            item: item.clone(),
            source: source.clone(),
            destination: destination.clone(),
            via_vault: steps.len() > 1,
        };
        Ok(Self {
            plan_id,
            membership,
            item,
            source,
            destination,
            revision,
            created_at,
            expires_at,
            steps,
            effects,
        })
    }

    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// The ordered transfer steps, bounded by [`MAX_MOVE_STEPS`].
    pub fn steps(&self) -> &[MoveStep] {
        &self.steps
    }

    /// The sanitized expected postcondition.
    pub fn effects(&self) -> &MoveEffects {
        &self.effects
    }

    /// The number of ordered steps; one for direct moves, two via the Vault.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }

    /// Re-checks every binding assumption against a preflight snapshot.
    ///
    /// Outcomes are reported in a fixed precedence so a caller never has to
    /// infer which assumption failed: expiry, revision, membership, plan
    /// locations, current source, resolved item, then resolved destination.
    pub fn binding(&self, input: &MutationPolicyInput) -> PlanBinding {
        if self.is_expired(input.now) {
            return PlanBinding::Expired;
        }
        if input.current_revision != self.revision {
            return PlanBinding::StaleRevision;
        }
        if input.membership != self.membership {
            return PlanBinding::WrongMembership;
        }
        if !supported_location(&self.source) || !supported_location(&self.destination) {
            return PlanBinding::UnsupportedLocation;
        }
        if !supported_location(&input.current_source) {
            return PlanBinding::UnsupportedSource;
        }
        if input.current_source != self.source {
            return PlanBinding::SourceMismatch;
        }
        match &input.resolved_item {
            Resolution::Missing => return PlanBinding::MissingResolution,
            Resolution::Ambiguous(_) => return PlanBinding::AmbiguousResolution,
            Resolution::Unique(item) if item != &self.item => return PlanBinding::WrongItem,
            Resolution::Unique(_) => {}
        }
        match &input.resolved_destination {
            Resolution::Missing => return PlanBinding::MissingDestination,
            Resolution::Ambiguous(_) => return PlanBinding::AmbiguousDestination,
            Resolution::Unique(location) => {
                if !supported_location(location) {
                    return PlanBinding::UnsupportedLocation;
                }
                if location != &self.destination {
                    return PlanBinding::WrongDestination;
                }
            }
        }
        PlanBinding::Valid
    }
}

fn supported_location(location: &ItemLocation) -> bool {
    matches!(location, ItemLocation::Vault | ItemLocation::Character(_))
}

/// The specific binding failure of one plan against a preflight snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanBinding {
    Valid,
    Expired,
    StaleRevision,
    WrongMembership,
    WrongItem,
    WrongDestination,
    MissingResolution,
    AmbiguousResolution,
    MissingDestination,
    AmbiguousDestination,
    UnsupportedLocation,
    UnsupportedSource,
    SourceMismatch,
}

/// The preflight snapshot a plan is re-checked against.
///
/// `resolved_item` and `resolved_destination` are the freshly re-resolved
/// identities, not the original request text, so a changed or ambiguous
/// resolution invalidates the plan instead of silently selecting a target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationPolicyInput {
    pub now: Timestamp,
    pub membership: MembershipId,
    pub current_revision: SyncRevision,
    pub current_source: ItemLocation,
    pub resolved_item: Resolution<ItemInstanceId>,
    pub resolved_destination: Resolution<ItemLocation>,
}

/// The sanitized outcome of one mutation policy evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationPolicyOutcome {
    Valid,
    Expired,
    StaleRevision,
    WrongMembership,
    WrongItem,
    WrongDestination,
    MissingResolution,
    AmbiguousResolution,
    MissingDestination,
    AmbiguousDestination,
    UnsupportedLocation,
    UnsupportedSource,
    SourceMismatch,
}

/// Evaluates one plan against a preflight snapshot through the shared policy.
pub fn evaluate_mutation(plan: &MovePlan, input: &MutationPolicyInput) -> MutationPolicyOutcome {
    match plan.binding(input) {
        PlanBinding::Valid => MutationPolicyOutcome::Valid,
        PlanBinding::Expired => MutationPolicyOutcome::Expired,
        PlanBinding::StaleRevision => MutationPolicyOutcome::StaleRevision,
        PlanBinding::WrongMembership => MutationPolicyOutcome::WrongMembership,
        PlanBinding::WrongItem => MutationPolicyOutcome::WrongItem,
        PlanBinding::WrongDestination => MutationPolicyOutcome::WrongDestination,
        PlanBinding::MissingResolution => MutationPolicyOutcome::MissingResolution,
        PlanBinding::AmbiguousResolution => MutationPolicyOutcome::AmbiguousResolution,
        PlanBinding::MissingDestination => MutationPolicyOutcome::MissingDestination,
        PlanBinding::AmbiguousDestination => MutationPolicyOutcome::AmbiguousDestination,
        PlanBinding::UnsupportedLocation => MutationPolicyOutcome::UnsupportedLocation,
        PlanBinding::UnsupportedSource => MutationPolicyOutcome::UnsupportedSource,
        PlanBinding::SourceMismatch => MutationPolicyOutcome::SourceMismatch,
    }
}

/// A validated, bounded plan lifetime policy.
///
/// The core owns the bound; a caller supplies the concrete duration. A zero or
/// over-long duration is rejected, and computing an expiry whose timestamp would
/// overflow fails with [`ErrorCategory::ClockOverflow`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanLifetime {
    ttl_seconds: u64,
}

/// Why a plan lifetime policy could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanLifetimeValidation {
    /// The duration is zero, which can never form an executable plan window.
    Zero,
    /// The duration exceeds [`MAX_PLAN_LIFETIME_SECONDS`].
    ExceedsMaximum,
}

impl PlanLifetime {
    /// The largest accepted lifetime.
    pub const MAX: Self = Self {
        ttl_seconds: MAX_PLAN_LIFETIME_SECONDS,
    };

    /// Validates one caller-supplied lifetime duration in seconds.
    pub fn new(ttl_seconds: u64) -> Result<Self, PlanLifetimeValidation> {
        if ttl_seconds == 0 {
            return Err(PlanLifetimeValidation::Zero);
        }
        if ttl_seconds > MAX_PLAN_LIFETIME_SECONDS {
            return Err(PlanLifetimeValidation::ExceedsMaximum);
        }
        Ok(Self { ttl_seconds })
    }

    /// The configured duration in seconds.
    pub fn ttl_seconds(self) -> u64 {
        self.ttl_seconds
    }

    /// Computes the bounded expiry for a plan created at `created_at`.
    ///
    /// A timestamp that would exceed the supported range fails with the existing
    /// [`ErrorCategory::ClockOverflow`] error.
    pub fn expires_at(self, created_at: Timestamp) -> Result<Timestamp, AppError> {
        created_at
            .seconds()
            .checked_add(self.ttl_seconds)
            .map(Timestamp::new)
            .ok_or_else(|| AppError::new(ErrorCategory::ClockOverflow))
    }
}

/// Convenience constructor for a character destination without provider terminology.
pub fn character_destination(id: CharacterId) -> ItemLocation {
    ItemLocation::Character(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::ItemLocation,
        error::{AppError, ErrorCategory},
        ids::{CharacterId, PlanId},
    };

    fn character(id: &str, name: Option<&str>, class: Option<&str>) -> Character {
        Character {
            id: CharacterId::new(id),
            name: name.map(str::to_owned),
            class: class.map(str::to_owned),
            race: None,
            gender: None,
            light: None,
        }
    }

    fn plan() -> MovePlan {
        MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(10),
            Timestamp::new(20),
        )
        .unwrap()
    }

    fn input(now: u64) -> MutationPolicyInput {
        MutationPolicyInput {
            now: Timestamp::new(now),
            membership: MembershipId::new("membership"),
            current_revision: SyncRevision::new(4),
            current_source: ItemLocation::Vault,
            resolved_item: Resolution::Unique(ItemInstanceId::new("item")),
            resolved_destination: Resolution::Unique(ItemLocation::Character(CharacterId::new(
                "character",
            ))),
        }
    }

    #[test]
    fn item_selectors_are_exact_or_validated_semantic_terms() {
        let exact = ItemSelector::instance(ItemInstanceId::new("instance-1"));
        assert!(exact.is_exact());
        assert_eq!(
            exact.instance_id().map(ItemInstanceId::as_str),
            Some("instance-1")
        );
        assert!(exact.semantic_term().is_none());

        let semantic = ItemSelector::semantic("  The   Last Word ").unwrap();
        assert!(!semantic.is_exact());
        assert_eq!(
            semantic.semantic_term().map(SearchTerm::as_str),
            Some("the last word")
        );
        assert!(semantic.instance_id().is_none());

        for invalid in ["", "   ", "bad\u{7}control"] {
            assert_eq!(
                ItemSelector::semantic(invalid),
                Err(AppError::new(ErrorCategory::ProviderValidation)),
                "item term {invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn destination_selectors_validate_and_match_labels_case_insensitively() {
        let warlock = character("warlock", Some("Nova"), Some("Warlock"));
        let hunter = character("hunter", None, Some("Hunter"));

        let vault = DestinationSelector::vault();
        assert!(vault.is_vault());
        assert!(!destination_matches(&vault, &warlock));

        let by_id = DestinationSelector::character(CharacterId::new("warlock"));
        assert!(destination_matches(&by_id, &warlock));
        assert!(!destination_matches(&by_id, &hunter));

        let by_class = DestinationSelector::character_label("  WARLOCK ").unwrap();
        assert!(destination_matches(&by_class, &warlock));
        assert!(!destination_matches(&by_class, &hunter));

        let by_name = DestinationSelector::character_label("nova").unwrap();
        assert!(destination_matches(&by_name, &warlock));
        assert!(!destination_matches(&by_name, &hunter));

        assert!(!destination_matches(
            &DestinationSelector::character_label("lock").unwrap(),
            &warlock
        ));

        for invalid in ["", "   ", "bad\u{7}control"] {
            assert_eq!(
                DestinationSelector::character_label(invalid),
                Err(AppError::new(ErrorCategory::ProviderValidation))
            );
        }
    }

    #[test]
    fn destination_resolution_reports_unique_missing_and_ambiguous() {
        let warlock = character("warlock", Some("Nova"), Some("Warlock"));
        let hunter = character("hunter", None, Some("Hunter"));
        let characters = vec![warlock.clone(), hunter];

        assert_eq!(
            resolve_destination(&DestinationSelector::vault(), &characters),
            Resolution::Unique(ItemLocation::Vault)
        );
        assert_eq!(
            resolve_destination(
                &DestinationSelector::character_label("warlock").unwrap(),
                &characters
            ),
            Resolution::Unique(ItemLocation::Character(CharacterId::new("warlock")))
        );
        assert_eq!(
            resolve_destination(
                &DestinationSelector::character(CharacterId::new("missing")),
                &characters
            ),
            Resolution::Missing
        );
        assert_eq!(
            resolve_destination(
                &DestinationSelector::character_label("titan").unwrap(),
                &characters
            ),
            Resolution::Missing
        );

        let two_warlocks = vec![
            warlock,
            character("warlock-two", Some("Nova Two"), Some("Warlock")),
        ];
        assert_eq!(
            resolve_destination(
                &DestinationSelector::character_label("warlock").unwrap(),
                &two_warlocks
            ),
            Resolution::Ambiguous(vec![
                ItemLocation::Character(CharacterId::new("warlock")),
                ItemLocation::Character(CharacterId::new("warlock-two")),
            ])
        );
    }

    #[test]
    fn explicit_intent_is_the_whole_authorization_fact() {
        assert!(MoveIntentAuthorization::Explicit.is_explicit());
        assert!(!MoveIntentAuthorization::Absent.is_explicit());
        assert!(!MoveIntentAuthorization::Ambiguous.is_explicit());

        assert_eq!(
            MoveIntentAuthorization::Explicit.authorize(),
            IntentAuthorizationOutcome::Authorized
        );
        assert_eq!(
            MoveIntentAuthorization::Absent.authorize(),
            IntentAuthorizationOutcome::MissingIntent
        );
        assert_eq!(
            MoveIntentAuthorization::Ambiguous.authorize(),
            IntentAuthorizationOutcome::AmbiguousIntent
        );
    }

    #[test]
    fn move_request_composes_validated_selectors_and_intent() {
        let request = MoveRequest {
            membership: MembershipId::new("membership"),
            item: ItemSelector::semantic("the last word").unwrap(),
            destination: DestinationSelector::character_label("Warlock").unwrap(),
            intent: MoveIntentAuthorization::Explicit,
        };
        assert_eq!(
            request.authorization(),
            IntentAuthorizationOutcome::Authorized
        );
        assert_eq!(
            request.item.semantic_term().map(SearchTerm::as_str),
            Some("the last word")
        );
        assert_eq!(
            request.destination,
            DestinationSelector::CharacterLabel(SearchTerm::new("warlock").unwrap())
        );
    }

    #[test]
    fn direct_moves_produce_one_ordered_step() {
        for (source, destination) in [
            (
                ItemLocation::Vault,
                ItemLocation::Character(CharacterId::new("warlock")),
            ),
            (
                ItemLocation::Character(CharacterId::new("warlock")),
                ItemLocation::Vault,
            ),
        ] {
            let steps = plan_steps(&source, &destination).unwrap();
            assert_eq!(steps.len(), 1, "{source:?} -> {destination:?} is direct");
            assert_eq!(steps[0].ordinal, 1);
            assert_eq!(steps[0].from, source);
            assert_eq!(steps[0].to, destination);
        }
    }

    #[test]
    fn character_to_character_uses_the_vault_exactly_once() {
        let source = ItemLocation::Character(CharacterId::new("source"));
        let destination = ItemLocation::Character(CharacterId::new("destination"));
        let steps = plan_steps(&source, &destination).unwrap();

        assert_eq!(steps.len(), MAX_MOVE_STEPS);
        assert_eq!(steps[0], MoveStep::new(1, source, ItemLocation::Vault));
        assert_eq!(steps[1], MoveStep::new(2, ItemLocation::Vault, destination));
        assert_eq!(
            steps
                .iter()
                .filter(|step| step.from == ItemLocation::Vault || step.to == ItemLocation::Vault)
                .count(),
            2
        );
    }

    #[test]
    fn plan_steps_reject_no_ops_and_unsupported_locations() {
        for (source, destination, expected) in [
            (
                ItemLocation::Vault,
                ItemLocation::Vault,
                MovePlanValidation::SameLocation,
            ),
            (
                ItemLocation::Character(CharacterId::new("same")),
                ItemLocation::Character(CharacterId::new("same")),
                MovePlanValidation::SameLocation,
            ),
            (
                ItemLocation::Other("unknown".into()),
                ItemLocation::Vault,
                MovePlanValidation::UnsupportedLocation,
            ),
            (
                ItemLocation::Vault,
                ItemLocation::Other("unknown".into()),
                MovePlanValidation::UnsupportedLocation,
            ),
        ] {
            assert_eq!(plan_steps(&source, &destination), Err(expected));
        }
    }

    #[test]
    fn plan_derives_bounded_steps_and_effects() {
        let direct = plan();
        assert_eq!(direct.step_count(), 1);
        assert_eq!(direct.steps()[0].ordinal, 1);
        assert!(!direct.effects().is_vault_mediated());
        assert_eq!(direct.effects().item, ItemInstanceId::new("item"));
        assert_eq!(direct.effects().source, ItemLocation::Vault);
        assert_eq!(
            direct.effects().destination,
            ItemLocation::Character(CharacterId::new("character"))
        );

        let mediated = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Character(CharacterId::new("source")),
            ItemLocation::Character(CharacterId::new("destination")),
            SyncRevision::new(4),
            Timestamp::new(10),
            Timestamp::new(20),
        )
        .unwrap();
        assert_eq!(mediated.step_count(), MAX_MOVE_STEPS);
        assert_eq!(mediated.steps()[0].to, ItemLocation::Vault);
        assert_eq!(mediated.steps()[1].from, ItemLocation::Vault);
        assert!(mediated.effects().is_vault_mediated());
        assert!(mediated.step_count() <= MAX_MOVE_STEPS);
    }

    #[test]
    fn plan_validates_time_window_lifetime_and_locations() {
        let invalid = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(21),
            Timestamp::new(20),
        );
        assert_eq!(invalid, Err(MovePlanValidation::InvalidTimestamps));

        let zero_duration = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(20),
            Timestamp::new(20),
        );
        assert_eq!(zero_duration, Err(MovePlanValidation::InvalidTimestamps));

        let at_bound = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(0),
            Timestamp::new(MAX_PLAN_LIFETIME_SECONDS),
        );
        assert!(at_bound.is_ok());

        let over_bound = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(0),
            Timestamp::new(MAX_PLAN_LIFETIME_SECONDS + 1),
        );
        assert_eq!(over_bound, Err(MovePlanValidation::LifetimeExceeded));

        for (source, destination, expected) in [
            (
                ItemLocation::Vault,
                ItemLocation::Vault,
                MovePlanValidation::SameLocation,
            ),
            (
                ItemLocation::Character(CharacterId::new("same")),
                ItemLocation::Character(CharacterId::new("same")),
                MovePlanValidation::SameLocation,
            ),
            (
                ItemLocation::Other("unknown".into()),
                ItemLocation::Vault,
                MovePlanValidation::UnsupportedLocation,
            ),
        ] {
            let invalid = MovePlan::new(
                PlanId::new("plan"),
                MembershipId::new("membership"),
                ItemInstanceId::new("item"),
                source,
                destination,
                SyncRevision::new(4),
                Timestamp::new(10),
                Timestamp::new(20),
            );
            assert_eq!(invalid, Err(expected));
        }
    }

    #[test]
    fn lifetime_policy_is_bounded_and_reports_overflow() {
        assert_eq!(PlanLifetime::new(0), Err(PlanLifetimeValidation::Zero));
        assert_eq!(
            PlanLifetime::new(MAX_PLAN_LIFETIME_SECONDS + 1),
            Err(PlanLifetimeValidation::ExceedsMaximum)
        );
        assert_eq!(
            PlanLifetime::new(MAX_PLAN_LIFETIME_SECONDS),
            Ok(PlanLifetime::MAX)
        );
        assert_eq!(PlanLifetime::MAX.ttl_seconds(), MAX_PLAN_LIFETIME_SECONDS);

        let lifetime = PlanLifetime::new(30).unwrap();
        assert_eq!(
            lifetime.expires_at(Timestamp::new(100)),
            Ok(Timestamp::new(130))
        );
        assert_eq!(
            PlanLifetime::MAX.expires_at(Timestamp::new(u64::MAX)),
            Err(AppError::new(ErrorCategory::ClockOverflow))
        );
    }

    #[test]
    fn expiry_boundary_is_valid_then_expired() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(&plan, &input(20)),
            MutationPolicyOutcome::Expired
        );
        assert_eq!(
            evaluate_mutation(&plan, &input(19)),
            MutationPolicyOutcome::Valid
        );
    }

    #[test]
    fn binding_rejects_revision_membership_item_and_source_mismatches() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    current_revision: SyncRevision::new(5),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::StaleRevision
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    membership: MembershipId::new("other"),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongMembership
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_item: Resolution::Unique(ItemInstanceId::new("other")),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongItem
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    current_source: ItemLocation::Character(CharacterId::new("other")),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::SourceMismatch
        );
    }

    #[test]
    fn binding_reports_resolution_outcomes_and_unsupported_locations() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_item: Resolution::Missing,
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::MissingResolution
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_item: Resolution::Ambiguous(vec![
                        ItemInstanceId::new("a"),
                        ItemInstanceId::new("b"),
                    ]),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::AmbiguousResolution
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_destination: Resolution::Missing,
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::MissingDestination
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_destination: Resolution::Ambiguous(vec![ItemLocation::Vault]),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::AmbiguousDestination
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_destination: Resolution::Unique(ItemLocation::Vault),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongDestination
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    resolved_destination: Resolution::Unique(ItemLocation::Other("unknown".into())),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::UnsupportedLocation
        );
        assert_eq!(
            evaluate_mutation(
                &plan,
                &MutationPolicyInput {
                    current_source: ItemLocation::Other("unknown".into()),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::UnsupportedSource
        );
    }

    #[test]
    fn binding_is_defensive_against_mutated_plan_locations() {
        let mut plan = plan();
        plan.source = ItemLocation::Other("unknown".into());
        assert_eq!(
            evaluate_mutation(&plan, &input(10)),
            MutationPolicyOutcome::UnsupportedLocation
        );
    }

    #[test]
    fn character_destination_helper_stays_provider_neutral() {
        assert_eq!(
            character_destination(CharacterId::new("warlock")),
            ItemLocation::Character(CharacterId::new("warlock"))
        );
    }
}
