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
//!
//! # Execution contracts
//!
//! This module also owns the contracts the application executor and the transfer
//! adapter share, still without performing any I/O itself. The remote seam is the
//! provider-neutral [`TransferGateway`] request/outcome pair; the local record is
//! the bounded, sanitized [`ExecutionEvidence`] the executor fills in one step at
//! a time; and the single-use [`ExecutionTicket`] is the only authorization to
//! attempt those steps. A ticket is minted from the current committed profile
//! projection whose revision still matches the plan, including state reused by
//! ordinary sync. It is neither `Clone` nor derivable from an outcome, error or
//! evidence. A changed local revision requires a new plan; a consumed ticket
//! cannot be replayed. Bungie remains the final validator. No automatic retry,
//! backoff, or recovery loop exists anywhere in this module.
//!
//! [`TransferGateway`]: crate::ports::TransferGateway
//! [`TransferOutcome`]: crate::ports::TransferOutcome

use crate::{
    domain::{Character, ItemLocation},
    error::{AppError, ErrorCategory},
    ids::{CharacterId, ItemInstanceId, MembershipId, PlanId, SyncRevision},
    ports::{TransferOutcome, TransferRejection},
    query::{SearchTerm, normalize_search_text},
    state::{ProfileSyncOutcome, Resolution, SourceMarker, Timestamp},
};

/// Maximum number of ordered steps one move plan may contain.
///
/// A direct move needs one step; a character-to-character move needs two so the
/// Vault is used exactly once as an intermediary. No plan may exceed this, which
/// keeps the result finite and reviewable.
pub const MAX_MOVE_STEPS: usize = 2;

/// Two Vault-mediated transfers followed by one equip action.
pub const MAX_MUTATION_STEPS: usize = 3;

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

/// The provider-neutral action performed by one mutation step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepAction {
    /// Transfer the plan's concrete item between two locations.
    Transfer {
        from: ItemLocation,
        to: ItemLocation,
    },
    /// Equip the plan's concrete item on one character.
    Equip { character: CharacterId },
}

/// One ordered action in a mutation plan.
///
/// The 1-based `ordinal` makes the order explicit. Move steps are derived
/// deterministically by [`plan_steps`]; the plan's concrete item is shared across
/// every step and therefore not repeated here.
#[derive(Clone, Eq, PartialEq)]
pub struct MutationStep {
    pub ordinal: u8,
    pub action: StepAction,
}

impl MutationStep {
    /// Builds one ordered transfer step.
    pub fn transfer(ordinal: u8, from: ItemLocation, to: ItemLocation) -> Self {
        Self {
            ordinal,
            action: StepAction::Transfer { from, to },
        }
    }
}

impl std::fmt::Debug for MutationStep {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Preserve the existing transfer diagnostic shape and identity redaction.
        match &self.action {
            StepAction::Transfer { from, to } => formatter
                .debug_struct("MoveStep")
                .field("ordinal", &self.ordinal)
                .field("from", from)
                .field("to", to)
                .finish(),
            StepAction::Equip { character } => formatter
                .debug_struct("EquipStep")
                .field("ordinal", &self.ordinal)
                .field("character", character)
                .finish(),
        }
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
) -> Result<Vec<MutationStep>, MovePlanValidation> {
    if !supported_location(source) || !supported_location(destination) {
        return Err(MovePlanValidation::UnsupportedLocation);
    }
    if source == destination {
        return Err(MovePlanValidation::SameLocation);
    }
    let steps =
        if matches!(source, ItemLocation::Vault) || matches!(destination, ItemLocation::Vault) {
            vec![MutationStep::transfer(
                1,
                source.clone(),
                destination.clone(),
            )]
        } else {
            vec![
                MutationStep::transfer(1, source.clone(), ItemLocation::Vault),
                MutationStep::transfer(2, ItemLocation::Vault, destination.clone()),
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
    steps: Vec<MutationStep>,
    effects: MoveEffects,
    requires_equipped: bool,
}

/// Semantic lock-state request; selector and explicit intent use the move rules.
/// Lock policy is independent of equip eligibility and location changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockRequest {
    pub membership: MembershipId,
    pub item: ItemSelector,
    pub locked: bool,
    pub intent: MoveIntentAuthorization,
}

/// Semantic equip request; selectors and explicit intent use the move rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipRequest {
    pub membership: MembershipId,
    pub item: ItemSelector,
    pub destination: DestinationSelector,
    pub intent: MoveIntentAuthorization,
}

impl EquipRequest {
    /// Reuses the move pipeline's selector and authorization contracts.
    pub fn as_move_request(&self) -> MoveRequest {
        MoveRequest {
            membership: self.membership.clone(),
            item: self.item.clone(),
            destination: self.destination.clone(),
            intent: self.intent,
        }
    }
}

/// Fresh equip preflight result. Neither variant authorizes remote execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipPreflight {
    Ready,
    AlreadySatisfied,
}

/// Revision-bound equip intent, with a location AND equipped postcondition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipPlan {
    plan: MovePlan,
    already_satisfied: bool,
}

impl EquipPlan {
    /// Builds transfers using the move routing rule, then appends Equip.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan_id: PlanId,
        membership: MembershipId,
        item: ItemInstanceId,
        source: ItemLocation,
        character: CharacterId,
        equipped: bool,
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
        if !supported_location(&source) {
            return Err(MovePlanValidation::UnsupportedLocation);
        }
        let destination = ItemLocation::Character(character.clone());
        let already_satisfied = source == destination && equipped;
        let mut steps = if source == destination {
            Vec::new()
        } else {
            plan_steps(&source, &destination)?
        };
        let via_vault = vault_mediated(&steps);
        if !already_satisfied {
            steps.push(MutationStep {
                ordinal: (steps.len() + 1) as u8,
                action: StepAction::Equip { character },
            });
        }
        debug_assert!(steps.len() <= MAX_MUTATION_STEPS);
        let effects = MoveEffects {
            item: item.clone(),
            source: source.clone(),
            destination: destination.clone(),
            via_vault,
        };
        Ok(Self {
            plan: MovePlan {
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
                requires_equipped: true,
            },
            already_satisfied,
        })
    }

    pub fn mutation_plan(&self) -> &MovePlan {
        &self.plan
    }

    pub fn steps(&self) -> &[MutationStep] {
        self.plan.steps()
    }

    pub fn via_vault(&self) -> bool {
        self.plan.effects().via_vault
    }

    /// Whether the planning observation already met both postconditions.
    pub fn is_already_satisfied(&self) -> bool {
        self.already_satisfied
    }

    /// Evaluates both postconditions on a fresh concrete instance.
    pub fn already_satisfied(&self, item: &crate::domain::ItemInstance) -> bool {
        item.instance_id == self.plan.item
            && item.location == self.plan.destination
            && item.equipped
    }
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
            via_vault: vault_mediated(&steps),
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
            requires_equipped: false,
        })
    }

    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// The ordered mutation steps, bounded by [`MAX_MUTATION_STEPS`].
    pub fn steps(&self) -> &[MutationStep] {
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

    /// True when the locally resolved location already satisfies the postcondition.
    ///
    /// The destination alone decides this, because a plan never binds a source
    /// equal to its destination. An already-satisfied plan is a *verified no-op*:
    /// the caller must validate the current committed projection under the
    /// ordinary freshness gate. Because local state satisfies intent, no transfer
    /// step may be issued and no [`ExecutionTicket`] may be spent. This predicate
    /// is a pure read, so it can never itself trigger a provider call.
    pub fn already_satisfied(&self, location: &ItemLocation) -> bool {
        !self.requires_equipped && location == &self.destination
    }

    /// Equip plans require equipped-state verification, not location alone.
    pub fn requires_equipped(&self) -> bool {
        self.requires_equipped
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

fn vault_mediated(steps: &[MutationStep]) -> bool {
    steps
        .iter()
        .filter(|step| matches!(step.action, StepAction::Transfer { .. }))
        .count()
        > 1
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

/// The bounded, sanitized result recorded for one executed plan step.
///
/// The record keeps the planned [`MutationStep`] next to the outcome observed for it,
/// so evidence proves *which* ordered step ran, not only how many. It carries no
/// credential, provider payload, or provider message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepRecord {
    pub step: MutationStep,
    pub outcome: StepOutcome,
}

/// The sanitized outcome of one executed plan step.
///
/// [`StepOutcome::Unattempted`] is the initial state of every planned step, and
/// the other four are the terminal shapes one attempt can produce. The variants
/// distinguish a step that applied, one the provider deterministically refused,
/// one that failed without applying, and one whose effect is unknown, so a caller
/// never has to collapse an uncertain step into a success or a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    /// The provider reported this step applied.
    Completed,
    /// The provider deterministically refused this step; it changed nothing.
    Rejected(TransferRejection),
    /// The equip provider deterministically refused this step.
    EquipRejected(crate::EquipRejection),
    /// Authentication was refused in a received response after dispatch.
    /// Records an attempt, never application; category is ProviderAuthentication.
    AuthenticationRefused,
    /// The provider failed transiently for this step; nothing was applied.
    Transient,
    /// Whether an already-issued step applied could not be determined.
    Uncertain,
    /// This step was never attempted.
    Unattempted,
}

impl StepOutcome {
    /// Maps one sanitized equip result without changing transfer evidence.
    pub const fn from_equip(outcome: crate::EquipOutcome) -> Self {
        match outcome {
            crate::EquipOutcome::AuthenticationRefused => Self::AuthenticationRefused,
            crate::EquipOutcome::Equipped => Self::Completed,
            crate::EquipOutcome::Rejected(reason) => Self::EquipRejected(reason),
            crate::EquipOutcome::Transient => Self::Transient,
            crate::EquipOutcome::Uncertain => Self::Uncertain,
        }
    }

    /// Maps one sanitized provider result to the step outcome it implies.
    pub const fn from_transfer(outcome: TransferOutcome) -> Self {
        match outcome {
            TransferOutcome::AuthenticationRefused => Self::AuthenticationRefused,
            TransferOutcome::Transferred => Self::Completed,
            TransferOutcome::Rejected(rejection) => Self::Rejected(rejection),
            TransferOutcome::Transient => Self::Transient,
            TransferOutcome::Uncertain => Self::Uncertain,
        }
    }

    /// True when this step outcome records an actual attempt.
    pub const fn is_attempted(self) -> bool {
        match self {
            Self::Unattempted => false,
            Self::Completed
            | Self::Rejected(_)
            | Self::EquipRejected(_)
            | Self::AuthenticationRefused
            | Self::Transient
            | Self::Uncertain => true,
        }
    }

    /// True only when this step applied and the sequence may advance.
    pub const fn continues(self) -> bool {
        match self {
            Self::Completed => true,
            Self::Rejected(_)
            | Self::EquipRejected(_)
            | Self::AuthenticationRefused
            | Self::Transient
            | Self::Uncertain
            | Self::Unattempted => false,
        }
    }

    /// True when this step was attempted and the sequence must stop.
    ///
    /// A refusal, a transient failure, and an indeterminate effect all stop a
    /// plan: a later step's precondition is the previous step's effect, so
    /// continuing after any of them would act on an unproven state.
    pub const fn stops_sequence(self) -> bool {
        self.is_attempted() && !self.continues()
    }

    /// True only when this step's effect could not be determined.
    pub const fn is_uncertain(self) -> bool {
        match self {
            Self::Uncertain => true,
            Self::Completed
            | Self::Rejected(_)
            | Self::EquipRejected(_)
            | Self::AuthenticationRefused
            | Self::Transient
            | Self::Unattempted => false,
        }
    }
}

/// Bounded, sanitized evidence of one attempted transfer sequence.
///
/// Evidence is built from exactly one [`MovePlan`], so its step list is the
/// plan's ordered steps and nothing else, and it is bounded by
/// [`MAX_MUTATION_STEPS`]. [`ExecutionEvidence::record`] advances the record one step
/// at a time, in plan order and only while every attempted step so far applied,
/// which is how "issue only ordered plan steps" and "stop at the first
/// non-completing step" are enforced by the record itself rather than by hopeful
/// caller discipline. The record contains typed identities and locations only: a
/// credential, a raw provider payload, or a provider message has no field here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionEvidence {
    plan_id: PlanId,
    item: ItemInstanceId,
    source: ItemLocation,
    destination: ItemLocation,
    steps: Vec<StepRecord>,
}

impl ExecutionEvidence {
    /// Builds fully unattempted evidence for exactly one plan's ordered steps.
    pub fn for_plan(plan: &MovePlan) -> Self {
        let steps: Vec<StepRecord> = plan
            .steps()
            .iter()
            .cloned()
            .map(|step| StepRecord {
                step,
                outcome: StepOutcome::Unattempted,
            })
            .collect();
        debug_assert!(steps.len() <= MAX_MUTATION_STEPS);
        Self {
            plan_id: plan.plan_id.clone(),
            item: plan.item.clone(),
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            steps,
        }
    }

    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    /// The concrete item this evidence is about.
    pub fn item(&self) -> &ItemInstanceId {
        &self.item
    }

    /// The location the plan expected the item to leave.
    pub fn source(&self) -> &ItemLocation {
        &self.source
    }

    /// The location the plan expected the item to reach.
    pub fn destination(&self) -> &ItemLocation {
        &self.destination
    }

    /// Every planned step with its recorded outcome, in plan order.
    pub fn steps(&self) -> &[StepRecord] {
        &self.steps
    }

    /// How many steps recorded an attempt.
    pub fn attempted_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|record| record.outcome.is_attempted())
            .count()
    }

    /// How many steps the provider reported applied.
    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|record| record.outcome.continues())
            .count()
    }

    /// How many steps have no recorded attempt.
    pub fn unattempted_count(&self) -> usize {
        self.steps.len() - self.attempted_count()
    }

    /// The ordinal of the next unattempted step, or `None` when none remains.
    pub fn next_ordinal(&self) -> Option<u8> {
        self.steps
            .iter()
            .find(|record| !record.outcome.is_attempted())
            .map(|record| record.step.ordinal)
    }

    /// True when every planned step reported that it applied.
    pub fn is_complete(&self) -> bool {
        self.completed_count() == self.steps.len()
    }

    /// True when the sequence applied at least one step but not every step.
    ///
    /// This is the honest partial-completion shape: some provider effect exists,
    /// so the move is neither a failure nor a success, and a caller must refresh
    /// before deciding anything about the item's location.
    pub fn is_partial(&self) -> bool {
        self.completed_count() > 0 && !self.is_complete()
    }

    /// The first attempted step that stopped the sequence, if any.
    pub fn stop_outcome(&self) -> Option<StepOutcome> {
        self.steps
            .iter()
            .find(|record| record.outcome.stops_sequence())
            .map(|record| record.outcome)
    }

    /// Records one outcome for the next unattempted step in plan order.
    ///
    /// Exactly one ordinal is acceptable at any point: the first step with no
    /// recorded attempt. Anything else is rejected with the sanitized
    /// [`ErrorCategory::StaleState`] category, because an out-of-order, repeated,
    /// or post-stop recording would describe a sequence that never ran. Recording
    /// [`StepOutcome::Unattempted`] is rejected with
    /// [`ErrorCategory::ProviderValidation`], because "no attempt" is the absence
    /// of a record rather than a result. No rejected call mutates the evidence.
    ///
    /// [`ErrorCategory::StaleState`]: crate::error::ErrorCategory::StaleState
    /// [`ErrorCategory::ProviderValidation`]: crate::error::ErrorCategory::ProviderValidation
    pub fn record(&mut self, ordinal: u8, outcome: StepOutcome) -> Result<(), AppError> {
        if !outcome.is_attempted() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        if self.stop_outcome().is_some() || self.next_ordinal() != Some(ordinal) {
            return Err(AppError::new(ErrorCategory::StaleState));
        }
        for record in &mut self.steps {
            if record.step.ordinal == ordinal {
                record.outcome = outcome;
                return Ok(());
            }
        }
        // `next_ordinal` proved the ordinal belongs to a planned step, so an
        // unmatched ordinal is unreachable and fails closed rather than silently
        // dropping the record.
        Err(AppError::new(ErrorCategory::StaleState))
    }
}

/// The single-use authorization to attempt one plan's ordered steps exactly once.
///
/// A ticket is minted only by [`ExecutionTicket::from_committed_projection`],
/// from the current committed profile projection that the unexpired plan was
/// bound to. Ordinary sync may reuse a fresh projection. Bungie remains the
/// final validator; local validation does not prove remote state is unchanged.
///
/// - The type is deliberately neither `Clone` nor `Copy`, so a completed attempt
///   cannot replay its authorization, and no accessor hands out a reusable one.
/// - No outcome, error, or evidence type produces a ticket, so an uncertain or
///   partial result has no path back to another provider call.
/// - The revision binding closes the remaining route: once an applied step has
///   advanced the committed projection, minting a second ticket for the same plan
///   against the current projection fails with [`ErrorCategory::StaleState`].
///   A further attempt against that revision requires a new plan, never replay
///   of a consumed ticket. An unchanged revision is not an attempt counter.
/// - The plan window is enforced here, not only in the preflight policy, so a
///   composition that skipped that policy cannot spend a ticket on expired state.
///
/// The ticket owns a copy of the plan's ordered steps so an executor cannot
/// invent, reorder, or extend them, and it carries no credential and no provider
/// value.
///
/// # Composition requirement (DTE-003)
///
/// The executor entry point must take this ticket **by value** and consume it for
/// the whole sequence, which is where the single-use guarantee is completed: this
/// contract exposes no borrowing attempt path, no replay, and no
/// refresh-and-retry helper, so a caller that keeps or copies the authorization
/// cannot exist. Until that executor consumes it by value, the guarantee is held
/// only by this type's construction and validation rules.
///
/// [`ErrorCategory::StaleState`]: crate::error::ErrorCategory::StaleState
#[derive(Debug, Eq, PartialEq)]
pub struct ExecutionTicket {
    plan_id: PlanId,
    membership: MembershipId,
    item: ItemInstanceId,
    source: ItemLocation,
    destination: ItemLocation,
    revision: SyncRevision,
    steps: Vec<MutationStep>,
}

impl ExecutionTicket {
    /// Mints a single-use ticket from the current committed projection.
    /// The caller must re-resolve and validate the plan against this projection.
    ///
    /// The plan must still be inside its window at the supplied `now`. The
    /// observation must describe the current committed profile projection and must
    /// describe the same membership and the same committed revision the plan was
    /// bound to. Any other input fails closed with the sanitized
    /// [`ErrorCategory::StaleState`] category, so a plan can never be attempted
    /// against an expired plan or local state that has already moved on.
    ///
    /// [`ErrorCategory::StaleState`]: crate::error::ErrorCategory::StaleState
    pub fn from_committed_projection(
        plan: &MovePlan,
        refreshed: &ProfileSyncOutcome,
        now: Timestamp,
    ) -> Result<Self, AppError> {
        if plan.is_expired(now) {
            return Err(AppError::new(ErrorCategory::StaleState));
        }
        let metadata = refreshed.metadata();
        if metadata.source != SourceMarker::Profile
            || metadata.membership_id != plan.membership
            || metadata.revision != plan.revision
        {
            return Err(AppError::new(ErrorCategory::StaleState));
        }
        Ok(Self {
            plan_id: plan.plan_id.clone(),
            membership: plan.membership.clone(),
            item: plan.item.clone(),
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            revision: plan.revision,
            steps: plan.steps().to_vec(),
        })
    }

    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    pub fn membership(&self) -> &MembershipId {
        &self.membership
    }

    /// The concrete item this ticket authorizes moving.
    pub fn item(&self) -> &ItemInstanceId {
        &self.item
    }

    /// The location the plan expected the item to leave.
    pub fn source(&self) -> &ItemLocation {
        &self.source
    }

    /// The location the plan expected the item to reach.
    pub fn destination(&self) -> &ItemLocation {
        &self.destination
    }

    /// The committed revision the plan and current projection agreed on.
    pub fn revision(&self) -> SyncRevision {
        self.revision
    }

    /// The ordered steps this ticket authorizes, bounded by [`MAX_MUTATION_STEPS`].
    pub fn steps(&self) -> &[MutationStep] {
        &self.steps
    }

    /// The number of authorized steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
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
        domain::{ItemLocation, SnapshotMetadata},
        error::{AppError, ErrorCategory},
        ids::{CharacterId, DefinitionVersion, MembershipType, PlanId},
    };

    fn character(id: &str, name: Option<&str>, class: Option<&str>) -> Character {
        Character {
            id: CharacterId::new(id),
            name: name.map(str::to_owned),
            class: class.map(str::to_owned),
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
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
            let StepAction::Transfer { from, to } = &steps[0].action else {
                panic!("expected transfer");
            };
            assert_eq!(from, &source);
            assert_eq!(to, &destination);
        }
    }

    #[test]
    fn character_to_character_uses_the_vault_exactly_once() {
        let source = ItemLocation::Character(CharacterId::new("source"));
        let destination = ItemLocation::Character(CharacterId::new("destination"));
        let steps = plan_steps(&source, &destination).unwrap();

        assert_eq!(steps.len(), MAX_MOVE_STEPS);
        assert_eq!(
            steps[0],
            MutationStep::transfer(1, source, ItemLocation::Vault)
        );
        assert_eq!(
            steps[1],
            MutationStep::transfer(2, ItemLocation::Vault, destination)
        );
        assert_eq!(
            steps
                .iter()
                .filter(|step| match &step.action {
                    StepAction::Transfer { from, to } => {
                        *from == ItemLocation::Vault || *to == ItemLocation::Vault
                    }
                    StepAction::Equip { .. } => false,
                })
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
            (
                ItemLocation::Postmaster(CharacterId::new("owner")),
                ItemLocation::Vault,
                MovePlanValidation::UnsupportedLocation,
            ),
            (
                ItemLocation::Vault,
                ItemLocation::Postmaster(CharacterId::new("owner")),
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
        let StepAction::Transfer { to, .. } = &mediated.steps()[0].action else {
            panic!("expected transfer");
        };
        assert_eq!(to, &ItemLocation::Vault);
        let StepAction::Transfer { from, .. } = &mediated.steps()[1].action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &ItemLocation::Vault);
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

    /// A committed projection whose revision is the plan's own binding revision.
    fn metadata(revision: u64) -> SnapshotMetadata {
        SnapshotMetadata {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(3),
            revision: SyncRevision::new(revision),
            definition_version: DefinitionVersion::new("definitions-v1"),
            checked_at: Timestamp::new(15),
            source: SourceMarker::Profile,
        }
    }

    /// One committed profile refresh at `revision`.
    fn refreshed(revision: u64) -> ProfileSyncOutcome {
        ProfileSyncOutcome::Refreshed {
            metadata: metadata(revision),
        }
    }

    /// One current timestamp strictly inside the `plan()` window (10..20).
    const NOW: Timestamp = Timestamp::new(15);

    /// One plan that must move through the Vault, with two ordered steps.
    fn vault_mediated_plan() -> MovePlan {
        MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Character(CharacterId::new("warlock")),
            ItemLocation::Character(CharacterId::new("hunter")),
            SyncRevision::new(4),
            Timestamp::new(10),
            Timestamp::new(20),
        )
        .unwrap()
    }

    #[test]
    fn step_outcomes_mirror_provider_results_and_stop_the_sequence() {
        assert_eq!(
            StepOutcome::from_transfer(TransferOutcome::Transferred),
            StepOutcome::Completed
        );
        assert_eq!(
            StepOutcome::from_transfer(TransferOutcome::Rejected(
                TransferRejection::DestinationFull
            )),
            StepOutcome::Rejected(TransferRejection::DestinationFull)
        );
        assert_eq!(
            StepOutcome::from_transfer(TransferOutcome::Transient),
            StepOutcome::Transient
        );
        assert_eq!(
            StepOutcome::from_transfer(TransferOutcome::Uncertain),
            StepOutcome::Uncertain
        );

        assert!(StepOutcome::Completed.is_attempted());
        assert!(StepOutcome::Completed.continues());
        assert!(!StepOutcome::Completed.stops_sequence());
        assert!(!StepOutcome::Unattempted.is_attempted());
        assert!(!StepOutcome::Unattempted.continues());
        assert!(!StepOutcome::Unattempted.stops_sequence());

        for outcome in [
            StepOutcome::Rejected(TransferRejection::Equipped),
            StepOutcome::Transient,
            StepOutcome::Uncertain,
        ] {
            assert!(outcome.is_attempted());
            assert!(!outcome.continues());
            assert!(
                outcome.stops_sequence(),
                "{outcome:?} must stop the sequence"
            );
        }

        assert!(StepOutcome::Uncertain.is_uncertain());
        assert!(!StepOutcome::Transient.is_uncertain());
    }

    #[test]
    fn evidence_starts_unattempted_and_bounded_to_the_plan_steps() {
        let direct = ExecutionEvidence::for_plan(&plan());
        assert_eq!(direct.plan_id(), &PlanId::new("plan"));
        assert_eq!(direct.item(), &ItemInstanceId::new("item"));
        assert_eq!(direct.source(), &ItemLocation::Vault);
        assert_eq!(
            direct.destination(),
            &ItemLocation::Character(CharacterId::new("character"))
        );
        assert_eq!(direct.steps().len(), 1);
        assert!(direct.steps().len() <= MAX_MOVE_STEPS);
        assert_eq!(direct.steps()[0].step.ordinal, 1);
        assert_eq!(direct.steps()[0].outcome, StepOutcome::Unattempted);
        assert_eq!(direct.next_ordinal(), Some(1));
        assert_eq!(direct.attempted_count(), 0);
        assert_eq!(direct.completed_count(), 0);
        assert_eq!(direct.unattempted_count(), 1);
        assert_eq!(direct.stop_outcome(), None);
        assert!(!direct.is_complete());
        assert!(!direct.is_partial());

        let mediated = ExecutionEvidence::for_plan(&vault_mediated_plan());
        assert_eq!(mediated.steps().len(), MAX_MOVE_STEPS);
        let StepAction::Transfer { from, to } = &mediated.steps()[0].step.action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &ItemLocation::Character(CharacterId::new("warlock")));
        assert_eq!(to, &ItemLocation::Vault);
        let StepAction::Transfer { to, .. } = &mediated.steps()[1].step.action else {
            panic!("expected transfer");
        };
        assert_eq!(to, &ItemLocation::Character(CharacterId::new("hunter")));
        assert_eq!(mediated.next_ordinal(), Some(1));
        assert_eq!(mediated.unattempted_count(), MAX_MOVE_STEPS);
    }

    #[test]
    fn evidence_records_only_the_next_unattempted_step_in_order() {
        let mut evidence = ExecutionEvidence::for_plan(&vault_mediated_plan());

        // "No attempt" is the absence of a record, never a recorded result.
        assert_eq!(
            evidence.record(1, StepOutcome::Unattempted),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
        // An unplanned or out-of-order ordinal describes a sequence that never ran.
        assert_eq!(
            evidence.record(2, StepOutcome::Completed),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        assert_eq!(
            evidence.record(3, StepOutcome::Completed),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        // No rejected call changed the evidence.
        assert_eq!(evidence.attempted_count(), 0);
        assert_eq!(evidence.next_ordinal(), Some(1));

        evidence
            .record(1, StepOutcome::Completed)
            .expect("first step");
        assert_eq!(evidence.attempted_count(), 1);
        assert_eq!(evidence.next_ordinal(), Some(2));
        // One step records one attempt; a replay is refused.
        assert_eq!(
            evidence.record(1, StepOutcome::Completed),
            Err(AppError::new(ErrorCategory::StaleState))
        );
    }

    #[test]
    fn evidence_stops_after_the_first_non_completing_step() {
        let mut evidence = ExecutionEvidence::for_plan(&vault_mediated_plan());
        evidence
            .record(1, StepOutcome::Completed)
            .expect("first step");
        evidence
            .record(2, StepOutcome::from_transfer(TransferOutcome::Uncertain))
            .expect("second step");

        assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Uncertain));
        assert_eq!(evidence.attempted_count(), 2);
        assert_eq!(evidence.completed_count(), 1);
        assert_eq!(evidence.unattempted_count(), 0);
        assert_eq!(evidence.next_ordinal(), None);
        assert!(evidence.is_partial());
        assert!(!evidence.is_complete());
        // A stopped sequence accepts no further step, not even a later one.
        assert_eq!(
            evidence.record(2, StepOutcome::Completed),
            Err(AppError::new(ErrorCategory::StaleState))
        );
    }

    #[test]
    fn evidence_separates_a_failed_second_step_from_a_complete_sequence() {
        let mut partial = ExecutionEvidence::for_plan(&vault_mediated_plan());
        partial
            .record(1, StepOutcome::Completed)
            .expect("first step");
        partial
            .record(2, StepOutcome::Rejected(TransferRejection::DestinationFull))
            .expect("second step");
        assert!(partial.is_partial());
        assert!(!partial.is_complete());
        assert_eq!(
            partial.stop_outcome(),
            Some(StepOutcome::Rejected(TransferRejection::DestinationFull))
        );

        let mut complete = ExecutionEvidence::for_plan(&vault_mediated_plan());
        complete
            .record(1, StepOutcome::Completed)
            .expect("first step");
        complete
            .record(2, StepOutcome::Completed)
            .expect("second step");
        assert!(complete.is_complete());
        assert!(!complete.is_partial());
        assert_eq!(complete.stop_outcome(), None);
        assert_eq!(complete.next_ordinal(), None);
    }

    #[test]
    fn a_first_step_refusal_is_not_partial_completion() {
        let mut evidence = ExecutionEvidence::for_plan(&plan());
        evidence
            .record(1, StepOutcome::Rejected(TransferRejection::NotTransferable))
            .expect("first step");

        assert!(!evidence.is_partial());
        assert!(!evidence.is_complete());
        assert_eq!(evidence.completed_count(), 0);
        assert_eq!(evidence.unattempted_count(), 0);
        assert_eq!(
            evidence.stop_outcome(),
            Some(StepOutcome::Rejected(TransferRejection::NotTransferable))
        );
    }

    #[test]
    fn a_ticket_requires_a_current_revision_bound_observation() {
        let plan = plan();

        // Ordinary sync may reuse the current fresh committed projection.
        assert!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &ProfileSyncOutcome::SkippedFresh {
                    metadata: metadata(4)
                },
                NOW
            )
            .is_ok()
        );
        // A non-profile observation cannot describe the committed projection.
        assert_eq!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &ProfileSyncOutcome::Refreshed {
                    metadata: SnapshotMetadata {
                        source: SourceMarker::Manifest,
                        ..metadata(4)
                    }
                },
                NOW
            ),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        // Another membership never authorizes this plan.
        assert_eq!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &ProfileSyncOutcome::Refreshed {
                    metadata: SnapshotMetadata {
                        membership_id: MembershipId::new("other"),
                        ..metadata(4)
                    }
                },
                NOW
            ),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        // A revision the plan was not bound to means state already moved on.
        assert_eq!(
            ExecutionTicket::from_committed_projection(&plan, &refreshed(5), NOW),
            Err(AppError::new(ErrorCategory::StaleState))
        );

        let ticket =
            ExecutionTicket::from_committed_projection(&plan, &refreshed(4), NOW).expect("ticket");
        assert_eq!(ticket.plan_id(), &PlanId::new("plan"));
        assert_eq!(ticket.membership(), &MembershipId::new("membership"));
        assert_eq!(ticket.item(), &ItemInstanceId::new("item"));
        assert_eq!(ticket.source(), &ItemLocation::Vault);
        assert_eq!(
            ticket.destination(),
            &ItemLocation::Character(CharacterId::new("character"))
        );
        assert_eq!(ticket.revision(), SyncRevision::new(4));
        assert_eq!(ticket.step_count(), 1);
        assert_eq!(ticket.steps(), plan.steps());
    }

    #[test]
    fn a_ticket_enforces_the_plan_window_itself() {
        let plan = plan();

        // A refreshed, revision-bound observation is not enough on its own: the
        // ticket enforces the plan window here, so a composition that skipped the
        // mandatory preflight policy cannot spend it on expired state.
        assert_eq!(
            ExecutionTicket::from_committed_projection(&plan, &refreshed(4), plan.expires_at()),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        assert_eq!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &refreshed(4),
                Timestamp::new(plan.expires_at().seconds() + 1)
            ),
            Err(AppError::new(ErrorCategory::StaleState))
        );
        // The last second inside the window still mints.
        assert_eq!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &refreshed(4),
                Timestamp::new(plan.expires_at().seconds() - 1)
            )
            .map(|ticket| ticket.step_count()),
            Ok(1)
        );
    }

    #[test]
    fn a_ticket_cannot_authorize_a_second_attempt_after_state_advanced() {
        let plan = vault_mediated_plan();
        let ticket =
            ExecutionTicket::from_committed_projection(&plan, &refreshed(4), NOW).expect("ticket");

        let mut evidence = ExecutionEvidence::for_plan(&plan);
        evidence
            .record(ticket.steps()[0].ordinal, StepOutcome::Completed)
            .expect("first step");
        assert_eq!(evidence.stop_outcome(), None);

        // A later fresh preflight with a new committed revision refuses the same
        // plan; another attempt against that state requires a new plan.
        assert_eq!(
            ExecutionTicket::from_committed_projection(
                &plan,
                &refreshed(plan.revision.value() + 1),
                NOW
            ),
            Err(AppError::new(ErrorCategory::StaleState))
        );
    }

    #[test]
    fn an_unchanged_committed_revision_can_mint_a_new_ticket() {
        // Revision binding is not an attempt counter. A new authorization may
        // use an unchanged projection, but never replay a consumed ticket.
        // Changed state needs a new plan (covered above).
        let plan = plan();
        assert!(ExecutionTicket::from_committed_projection(&plan, &refreshed(4), NOW).is_ok());
    }

    #[test]
    fn an_already_satisfied_destination_is_the_only_verified_no_op() {
        let plan = plan();
        assert!(plan.already_satisfied(&ItemLocation::Character(CharacterId::new("character"))));
        assert!(!plan.already_satisfied(&ItemLocation::Vault));
        assert!(!plan.already_satisfied(&ItemLocation::Character(CharacterId::new("warlock"))));
        assert!(!plan.already_satisfied(&ItemLocation::other("future-location")));
    }

    #[test]
    fn a_direct_plan_completes_after_its_only_step_applies() {
        let mut evidence = ExecutionEvidence::for_plan(&plan());
        evidence
            .record(1, StepOutcome::Completed)
            .expect("only step");

        assert!(evidence.is_complete());
        assert!(!evidence.is_partial());
        assert_eq!(evidence.attempted_count(), 1);
        assert_eq!(evidence.completed_count(), 1);
        assert_eq!(evidence.unattempted_count(), 0);
        assert_eq!(evidence.stop_outcome(), None);
        assert_eq!(evidence.next_ordinal(), None);
    }
}
