//! Shared application mutation service: semantic move planning and the
//! mandatory preflight gate.
//!
//! # Scope
//!
//! [`MutationService`] is the single adapter-neutral path that turns a
//! validated semantic [`MoveRequest`] into either a bounded [`MovePlan`] or one
//! sanitized typed [`MutationError`]. It composes the existing
//! [`ProfileSyncService`] for freshness and the core-owned pure mutation
//! contracts for policy. It holds no transfer, execution, or provider-mutation
//! seam, so planning can only read the committed projection and can never cause
//! a remote mutation as a side effect.
//!
//! # Explicit intent
//!
//! Every entry point evaluates [`MoveRequest::authorization`] *before* any side
//! effect, including a profile refresh. Absent or ambiguous intent fails closed
//! with a typed error, so omitting intent can never reach the provider through
//! planning. Explicit intent is the whole authorization fact: no preview token,
//! confirmation value, approval ceremony, or second call is accepted or
//! required, which is what lets the MCP adapter invoke a single `move_item`
//! operation directly.
//!
//! # Item and destination resolution
//!
//! Resolution never guesses. An exact [`ItemSelector::Instance`] is resolved by
//! identity through [`SearchRepository::lookup_instance`]. A semantic
//! [`ItemSelector::Semantic`] term is resolved through the existing bounded
//! definition search and then the bounded instance search: a missing, truncated,
//! or multiple match fails closed instead of ranking candidates. A
//! [`DestinationSelector`] resolves through the shared core
//! [`resolve_destination`] rule against the committed characters, so the Vault,
//! one exact character, and one normalized case-insensitive label are the only
//! accepted destinations and an unknown or ambiguous one is rejected.
//!
//! # Planning (DMP-003)
//!
//! [`MutationService::plan_move`] authorizes intent, applies the ordinary
//! freshness gate (an already-fresh projection is not re-fetched), resolves the
//! item and destination, and builds one bounded [`MovePlan`] through the shared
//! [`PlanLifetime`] policy and the injected [`PlanIdGenerator`] seam. The plan
//! binds the concrete item, its current source, the destination, the membership,
//! the committed revision, the creation time, and the bounded expiry. The
//! bounded expiry is computed before a plan identifier is spent, so an
//! overflowing clock never consumes one.
//!
//! # Preflight (DMP-004)
//!
//! [`MutationService::preflight_move`] authorizes intent again and forces
//! [`ProfileSyncReason::Preflight`], which refreshes regardless of freshness,
//! then re-resolves the stable item identity, the destination, the current
//! source, the membership, and the committed revision against the refreshed
//! projection and evaluates the shared core [`evaluate_mutation`] policy. Every
//! non-valid [`MutationPolicyOutcome`] is mapped to a specific typed error
//! instead of being reported as an approximate or aggregate failure.
//!
//! Preflight then re-binds the *request's* own selectors: the requested item and
//! destination are resolved again against the same refreshed committed
//! projection and must match the plan's item and destination uniquely. The
//! plan-anchored policy evaluation proves the plan is still internally
//! consistent; this second resolution proves the caller's intent refers to that
//! same plan, so a request for one item can never preflight a plan for another.
//! A missing, ambiguous, or mismatched selector is rejected with the existing
//! typed item/destination outcome.
//!
//! The current source is only *known* when the item re-resolves uniquely; when
//! the item cannot be re-resolved, the plan's own source is submitted so the
//! shared policy reports the item resolution failure instead of a misleading
//! source mismatch. The plan's revision is never rebound: any change to the
//! committed projection advances its revision, and the policy invalidates the
//! plan before an execution could act on changed assumptions.
//!
//! # Composed entry point
//!
//! [`MutationService::prepare_move`] is the single shared adapter entry: it
//! plans and then preflights, so a presentation adapter cannot obtain a plan
//! without passing the mandatory gate. The two phases stay independently
//! callable because they own distinct contracts.
//!
//! # Execution (DTE-003)
//!
//! [`MutationService::authorize_execution`] is the mandatory fresh preflight
//! gate for one attempt. It validates intent first, rejects an expired plan or a
//! wrong-membership request *before* any provider read, and only then forces the
//! same acquisition and shared policy evaluation the preflight uses. It either
//! reports a *verified no-op* when the freshly observed concrete item already
//! sits at the plan's destination, or mints the single-use [`ExecutionTicket`]
//! that authorizes exactly one ordered attempt sequence.
//!
//! [`MutationService::execute_move`] consumes that ticket **by value** and
//! issues only the plan's ordered steps through the injected [`TransferGateway`],
//! stopping at the first step that does not complete and recording bounded
//! [`ExecutionEvidence`]. Success returns that evidence directly; every failure
//! returns an [`ExecutionFailure`] that still carries the evidence observed so
//! far, so a later pre-dispatch failure keeps an earlier applied step instead of
//! discarding it. A first-step failure before dispatch has no attempted step and
//! so honestly reports that no remote mutation occurred, while a post-dispatch
//! outcome that cannot be determined is recorded as typed step uncertainty.
//! [`ExecutionFailure::requires_post_action_refresh`] is the single predicate
//! the DTE-004 orchestration uses to find every path that must refresh. The
//! executor seam itself stays deliberately narrow: [`MutationService::execute_move`]
//! never refreshes after acting and never verifies the final destination on its
//! own, which is what keeps one fresh preflight bound to exactly one ordered
//! attempt.
//!
//! # Acknowledgement (IAX-001)
//!
//! [`MutationService::execute_move_verified`] retains its historical name but
//! now classifies provider receipts without a post-action request. Every planned
//! step must be acknowledged for [`MoveExecutionOutcome::Applied`]; uncertainty,
//! refusal and partial progress remain incomplete. Dispatched attempts durably
//! invalidate freshness without guessing inventory content. The next ordinary
//! read forces sync. Already-satisfied fresh preflight remains a verified no-op.

use std::fmt;

use crate::sync::ProfileSyncService;
use destiny_core::{
    AppError, DefinitionSearchQuery, DestinationSelector, DestinyMembership, ErrorCategory,
    ExecutionEvidence, ExecutionTicket, FreshnessPolicy, InstanceSearchQuery,
    IntentAuthorizationOutcome, InventoryRepository, InventorySnapshot, ItemHashFilter,
    ItemInstance, ItemInstanceId, ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT,
    MovePlan, MovePlanValidation, MoveRequest, MutationPolicyInput, MutationPolicyOutcome,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncOutcome, ProfileSyncReason,
    Resolution, SearchRepository, StepAction, StepOutcome, SyncRevision, Timestamp,
    TransferGateway, TransferRequest, evaluate_mutation, resolve_destination,
};

/// The sanitized, typed failure of semantic move planning or preflight.
///
/// The record carries no item identifier, membership identifier, provider
/// payload, or adapter value, so it is safe to present. Every variant maps to
/// one shared [`ErrorCategory`] through [`MutationError::category`], and one
/// variant preserves a propagated seam failure category without losing it.
///
/// The core policy owns a fixed outcome precedence, so several of its defensive
/// outcomes are only reachable from an invalid or contract-violating projection
/// rather than from a healthy one. They are still mapped here, distinctly and
/// exhaustively, because a sanitized policy result must never be collapsed into
/// a generic failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationError {
    /// A seam failure propagated unchanged; the sanitized category is kept.
    Application(ErrorCategory),
    /// The request carried no explicit intent to move one item.
    MissingIntent,
    /// The request did not attribute intent to exactly one move.
    AmbiguousIntent,
    /// The requested item matched nothing in the committed projection.
    MissingItem,
    /// The requested item matched more than one concrete instance.
    AmbiguousItem,
    /// The requested destination matched nothing in the committed projection.
    MissingDestination,
    /// The requested destination matched more than one character.
    AmbiguousDestination,
    /// The requested destination is already the item's current location.
    SameLocation,
    /// A plan endpoint lies outside the Vault/character contract.
    UnsupportedLocation,
    /// The item's re-resolved current location is unsupported.
    UnsupportedSource,
    /// The item is no longer at the location the plan recorded.
    SourceMismatch,
    /// The plan's bounded lifetime elapsed before preflight.
    Expired,
    /// The post-action observation timestamp precedes the preflight timestamp.
    InvalidTimestamps,
    /// The committed projection changed after the plan was created.
    StaleRevision,
    /// The plan binds a different membership than the preflight request.
    WrongMembership,
    /// The plan no longer matches the requested item.
    WrongItem,
    /// The plan no longer matches the requested destination.
    WrongDestination,
    /// The planned item no longer resolves in the committed projection.
    MissingResolution,
    /// The planned item resolves ambiguously in the committed projection.
    AmbiguousResolution,
    /// The shared plan validation rejected the requested move.
    InvalidPlan,
    /// A refresh succeeded but no committed projection became visible.
    InvalidProjection,
    InvalidEquipTarget,
    EquippedOnOtherCharacter,
    WrongClass,
    UnknownClass,
    /// Missing rarity information prevents a safe exotic-limit decision.
    UnknownRarity,
    /// An equipped neighbor cannot be classified for the exotic-limit check.
    UnknownNeighborSlot,
    NotEquippable,
    ExoticConflict,
    EquipNotYetSupported,
}

impl MutationError {
    /// The shared sanitized category for this failure.
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::Application(category) => category,
            Self::MissingIntent => ErrorCategory::AuthorizationRequired,
            Self::AmbiguousIntent
            | Self::AmbiguousItem
            | Self::AmbiguousDestination
            | Self::AmbiguousResolution => ErrorCategory::AmbiguousMatch,
            Self::MissingItem
            | Self::MissingDestination
            | Self::MissingResolution
            | Self::SourceMismatch
            | Self::Expired
            | Self::InvalidTimestamps
            | Self::StaleRevision
            | Self::WrongMembership
            | Self::WrongItem
            | Self::WrongDestination => ErrorCategory::StaleState,
            Self::InvalidEquipTarget
            | Self::EquippedOnOtherCharacter
            | Self::WrongClass
            | Self::UnknownClass
            | Self::UnknownRarity
            | Self::UnknownNeighborSlot
            | Self::NotEquippable
            | Self::ExoticConflict
            | Self::EquipNotYetSupported
            | Self::SameLocation
            | Self::UnsupportedLocation
            | Self::UnsupportedSource => ErrorCategory::UnsupportedOperation,
            Self::InvalidPlan | Self::InvalidProjection => ErrorCategory::LocalStorage,
        }
    }

    /// Maps one shared core policy outcome to its specific typed failure.
    ///
    /// [`MutationPolicyOutcome::Valid`] has no failure meaning; it maps to
    /// [`MutationError::InvalidPlan`] only so the mapping stays total, and
    /// callers that already handled validity never observe it.
    pub const fn from_policy_outcome(outcome: MutationPolicyOutcome) -> Self {
        match outcome {
            MutationPolicyOutcome::Valid => Self::InvalidPlan,
            MutationPolicyOutcome::Expired => Self::Expired,
            MutationPolicyOutcome::StaleRevision => Self::StaleRevision,
            MutationPolicyOutcome::WrongMembership => Self::WrongMembership,
            MutationPolicyOutcome::WrongItem => Self::WrongItem,
            MutationPolicyOutcome::WrongDestination => Self::WrongDestination,
            MutationPolicyOutcome::MissingResolution => Self::MissingResolution,
            MutationPolicyOutcome::AmbiguousResolution => Self::AmbiguousResolution,
            MutationPolicyOutcome::MissingDestination => Self::MissingDestination,
            MutationPolicyOutcome::AmbiguousDestination => Self::AmbiguousDestination,
            MutationPolicyOutcome::UnsupportedLocation => Self::UnsupportedLocation,
            MutationPolicyOutcome::UnsupportedSource => Self::UnsupportedSource,
            MutationPolicyOutcome::SourceMismatch => Self::SourceMismatch,
        }
    }
}

impl From<AppError> for MutationError {
    fn from(error: AppError) -> Self {
        Self::Application(error.category())
    }
}

impl From<MutationError> for AppError {
    fn from(error: MutationError) -> Self {
        Self::new(error.category())
    }
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Application(category) => write!(formatter, "{}", AppError::new(*category)),
            Self::MissingIntent => {
                formatter.write_str("explicit intent to move one item is required")
            }
            Self::AmbiguousIntent => {
                formatter.write_str("intent does not identify exactly one move")
            }
            Self::MissingItem => {
                formatter.write_str("the requested item is not in the committed projection")
            }
            Self::AmbiguousItem => {
                formatter.write_str("the requested item matched more than one item")
            }
            Self::MissingDestination => {
                formatter.write_str("the requested destination could not be resolved")
            }
            Self::AmbiguousDestination => {
                formatter.write_str("the requested destination matched more than one character")
            }
            Self::SameLocation => {
                formatter.write_str("the item is already at the requested destination")
            }
            Self::UnsupportedLocation => {
                formatter.write_str("the source or destination location is unsupported")
            }
            Self::UnsupportedSource => {
                formatter.write_str("the item's current location is unsupported")
            }
            Self::SourceMismatch => {
                formatter.write_str("the item is no longer at its planned source")
            }
            Self::Expired => formatter.write_str("the move plan has expired"),
            Self::InvalidTimestamps => {
                formatter.write_str("the post-action timestamp precedes the preflight timestamp")
            }
            Self::StaleRevision => {
                formatter.write_str("local state changed after the plan was created")
            }
            Self::WrongMembership => {
                formatter.write_str("the plan belongs to a different membership")
            }
            Self::WrongItem => formatter.write_str("the plan no longer matches the requested item"),
            Self::WrongDestination => {
                formatter.write_str("the plan no longer matches the requested destination")
            }
            Self::MissingResolution => {
                formatter.write_str("the planned item is no longer in the committed projection")
            }
            Self::AmbiguousResolution => {
                formatter.write_str("the planned item resolved ambiguously")
            }
            Self::InvalidPlan => formatter.write_str("the move plan could not be constructed"),
            Self::InvalidProjection => formatter.write_str("local projection state is unavailable"),
            Self::InvalidEquipTarget => {
                formatter.write_str("equip requires a character, not the Vault")
            }
            Self::EquippedOnOtherCharacter => formatter
                .write_str("item is equipped on another character; freeing it is out of scope"),
            Self::WrongClass => {
                formatter.write_str("item is not equippable by the target character's class")
            }
            Self::UnknownClass => {
                formatter.write_str("item or target class restriction is unknown")
            }
            Self::UnknownRarity => formatter
                .write_str("item rarity is unknown; exotic compatibility cannot be determined"),
            Self::UnknownNeighborSlot => formatter.write_str(
                "equipped neighbor slot is unknown; exotic compatibility cannot be determined",
            ),
            Self::NotEquippable => {
                formatter.write_str("item has no supported weapon or armor equipment slot")
            }
            Self::ExoticConflict => formatter
                .write_str("another exotic is equipped in the same category in a different slot"),
            Self::EquipNotYetSupported => {
                formatter.write_str("equip execution is not yet supported")
            }
        }
    }
}

impl std::error::Error for MutationError {}

/// The result of the mandatory fresh execution preflight gate.
///
/// The gate cannot itself mutate the provider, and it refuses an expired plan or
/// a request bound to another membership before it produces either variant, so
/// there are exactly two terminal shapes: either the freshly refreshed
/// projection already proved the requested end state, or the gate produced the
/// single-use ticket that authorizes exactly one ordered attempt sequence.
#[derive(Debug, Eq, PartialEq)]
pub enum ExecutionAuthorization {
    /// The mandatory fresh preflight proved the concrete item already sits at
    /// the plan's destination, so the requested end state already holds. No
    /// transfer step is issued and no [`ExecutionTicket`] is spent; the carried
    /// evidence is the plan's bounded, all-unattempted step skeleton.
    VerifiedNoOp(ExecutionEvidence),
    /// The mandatory fresh preflight authorized exactly one ordered attempt.
    /// The ticket is not `Clone`, so it can be consumed at most once.
    Authorized(ExecutionTicket),
}

/// The sanitized failure of one execution attempt together with the bounded step
/// evidence observed before it stopped.
///
/// The record always carries evidence for the attempted sequence, so a caller
/// can never observe a partial sequence and then lose the account of what already
/// happened: a second-step pre-dispatch failure still reports the first step's
/// completion instead of collapsing back to a bare error. The evidence is
/// bounded by the plan exactly as [`ExecutionEvidence`] is.
///
/// [`ExecutionFailure::requires_post_action_refresh`] is the single predicate
/// [`MutationService::execute_move_verified`] uses to decide whether the
/// DTE-004 post-action refresh is mandatory: it is true exactly when at least one
/// step reached the provider, so every path that must refresh is observable and a
/// first-step pre-dispatch failure honestly reports that no remote mutation
/// occurred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionFailure {
    error: MutationError,
    evidence: Box<ExecutionEvidence>,
}

impl ExecutionFailure {
    /// Builds one bounded failure record.
    pub fn new(error: MutationError, evidence: ExecutionEvidence) -> Self {
        Self {
            error,
            evidence: Box::new(evidence),
        }
    }

    /// The sanitized typed error that stopped the attempt.
    pub fn error(&self) -> MutationError {
        self.error
    }

    /// The bounded evidence observed before the attempt stopped.
    pub fn evidence(&self) -> &ExecutionEvidence {
        &self.evidence
    }

    /// True when at least one step reached the provider, so a post-action
    /// refresh is mandatory before any further decision.
    ///
    /// A first-step failure before dispatch has no attempted step, so this is
    /// false and no remote mutation occurred; a failure after an earlier step
    /// applied has a nonzero attempted count, so the refreshed state must be
    /// read before anything else.
    pub fn requires_post_action_refresh(&self) -> bool {
        self.evidence.attempted_count() > 0
    }
}

/// The honest, sanitized account of a dispatched sequence that did not complete
/// without proving the concrete item's post-action state.
///
/// The record preserves the bounded [`ExecutionEvidence`] received before the
/// attempt stopped, the expected location derived from completed receipts
/// (not an observation), and the pre-dispatch provider error when the sequence
/// stopped on one rather than on a classified step outcome. The concept is
/// deliberately *incomplete* rather than *partial*: zero, one, or every planned
/// step may have applied before the sequence stopped, and
/// [`IncompleteExecution::is_partial`] is what reports whether any step applied.
/// It never claims a rollback: an applied step stays applied, so an item may
/// honestly be stranded at an intermediate location such as the Vault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncompleteExecution {
    error: Option<MutationError>,
    expected_location: ItemLocation,
    evidence: Box<ExecutionEvidence>,
}

impl IncompleteExecution {
    fn new(
        error: Option<MutationError>,
        expected_location: ItemLocation,
        evidence: ExecutionEvidence,
    ) -> Self {
        Self {
            error,
            expected_location,
            evidence: Box::new(evidence),
        }
    }

    /// The pre-dispatch provider error that stopped the sequence, when the
    /// sequence stopped on an error instead of on a classified step outcome.
    pub fn error(&self) -> Option<MutationError> {
        self.error
    }

    /// Receipt-derived expected location, not a post-action observation.
    pub fn expected_location(&self) -> &ItemLocation {
        &self.expected_location
    }

    /// Compatibility accessor for the receipt-derived expected location.
    /// This is not an observation; presentation must use `expected_location`.
    pub fn observed(&self) -> &ItemLocation {
        self.expected_location()
    }

    /// The bounded step evidence observed before the attempt stopped.
    pub fn evidence(&self) -> &ExecutionEvidence {
        &self.evidence
    }

    /// The classified step outcome that stopped the sequence, when there was one.
    pub fn stop_outcome(&self) -> Option<StepOutcome> {
        self.evidence.stop_outcome()
    }

    /// True when at least one planned step reported that it applied.
    ///
    /// A false value means no step was acknowledged as completed, not that
    /// no provider effect occurred. This predicate does not establish success
    /// or observed location; uncertainty is preserved and no rollback is claimed.
    pub fn is_partial(&self) -> bool {
        self.evidence.is_partial()
    }
}

/// Why the mandatory post-action verification could not prove the concrete item
/// at the requested destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationFailureReason {
    /// The forced post-action refresh, or a read of the refreshed projection,
    /// failed, so no observation could be made.
    RefreshFailed,
    /// The item did not resolve to exactly one concrete instance in the
    /// refreshed projection.
    Unresolved,
    /// The item's location or required equipped state contradicts the progress
    /// the dispatched sequence reported.
    Elsewhere,
}

/// The sanitized verification failure and the bounded evidence it preserves.
///
/// A dispatched sequence is never discarded when the post-action verification
/// cannot prove the destination: the reason, the concrete observed location when
/// there is one, the pre-dispatch or refresh error that applies, and the bounded
/// step evidence all survive. No rollback is claimed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationFailure {
    reason: VerificationFailureReason,
    observed: Option<ItemLocation>,
    error: Option<MutationError>,
    evidence: Box<ExecutionEvidence>,
}

impl VerificationFailure {
    fn new(
        reason: VerificationFailureReason,
        observed: Option<ItemLocation>,
        error: Option<MutationError>,
        evidence: ExecutionEvidence,
    ) -> Self {
        Self {
            reason,
            observed,
            error,
            evidence: Box::new(evidence),
        }
    }

    /// Why the destination could not be verified.
    pub fn reason(&self) -> VerificationFailureReason {
        self.reason
    }

    /// The concrete location the refreshed projection observed, when the item
    /// resolved uniquely to one.
    pub fn observed(&self) -> Option<&ItemLocation> {
        self.observed.as_ref()
    }

    /// The sanitized error that made verification impossible, when one applies.
    ///
    /// This is the refresh or projection-read failure for
    /// [`VerificationFailureReason::RefreshFailed`], the pre-dispatch provider
    /// error for a contradicted sequence that stopped on one, and `None`
    /// otherwise.
    pub fn error(&self) -> Option<MutationError> {
        self.error
    }

    /// The bounded step evidence observed before the attempt stopped.
    pub fn evidence(&self) -> &ExecutionEvidence {
        &self.evidence
    }
}

/// Provider-confirmed steps whose post-action observation is still stale.
#[derive(Debug, Eq, PartialEq)]
pub struct PendingProviderRefresh {
    observed: ItemLocation,
    evidence: ExecutionEvidence,
}

impl PendingProviderRefresh {
    /// The stale, unconfirmed location from the unchanged projection.
    pub fn observed(&self) -> &ItemLocation {
        &self.observed
    }

    /// Every planned step was confirmed completed by the provider.
    pub fn evidence(&self) -> &ExecutionEvidence {
        &self.evidence
    }
}

/// The typed, sanitized final outcome of one move/equip attempt.
///
/// Full acknowledgement yields `Applied`, not an observed destination. Fresh
/// already-satisfied preflight yields `VerifiedNoOp`. Refused, partial and
/// uncertain attempts yield `IncompleteExecution`, preserving ordered receipts.
/// Legacy verification variants remain for compatibility; no post-action
/// provider verification is performed. Cache invalidation failure fails closed.
/// A first-step pre-dispatch error remains an error without remote mutation.
#[derive(Debug, Eq, PartialEq)]
pub enum MoveExecutionOutcome {
    /// Every planned step was acknowledged by Bungie; no post-action read.
    Applied(ExecutionEvidence),
    /// The refreshed committed projection proves the concrete item at the
    /// requested destination.
    VerifiedSuccess(ExecutionEvidence),
    /// The mandatory fresh preflight proved the concrete item already at the
    /// destination, so no transfer step was dispatched.
    VerifiedNoOp(ExecutionEvidence),
    /// All steps were provider-confirmed, but the unchanged profile has not
    /// confirmed the target. Do not retry; synchronize later to confirm.
    AppliedPendingProviderRefresh(PendingProviderRefresh),
    /// A dispatched sequence stopped short and the refreshed projection is
    /// consistent with the progress it reported.
    IncompleteExecution(IncompleteExecution),
    /// A dispatched sequence occurred but the refreshed projection cannot prove
    /// the concrete item at the requested destination.
    VerificationFailed(VerificationFailure),
}

/// Plans and preflights one semantic `move_item` request.
///
/// The service owns the sync orchestration and the plan identifier seam by
/// value, exactly as [`crate::queries::QueryService`] owns its sync service, so
/// both presentation adapters share one policy path instead of each
/// re-implementing resolution, binding, or preflight.
pub struct MutationService<G, R, P> {
    sync: ProfileSyncService<G, R>,
    plan_ids: P,
}

impl<G, R, P> MutationService<G, R, P>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository,
    P: PlanIdGenerator,
{
    pub fn new(sync: ProfileSyncService<G, R>, plan_ids: P) -> Self {
        Self { sync, plan_ids }
    }

    /// Borrows the composed synchronization service.
    pub fn sync(&self) -> &ProfileSyncService<G, R> {
        &self.sync
    }

    /// Mutably borrows the composed synchronization service.
    pub fn sync_mut(&mut self) -> &mut ProfileSyncService<G, R> {
        &mut self.sync
    }

    /// Borrows the repository so callers can read or seed the projection.
    pub fn repository(&self) -> &R {
        self.sync.repository()
    }

    /// Mutably borrows the repository for test or composition setup.
    pub fn repository_mut(&mut self) -> &mut R {
        self.sync.repository_mut()
    }

    /// Borrows the injected plan identifier generator.
    pub fn plan_ids(&self) -> &P {
        &self.plan_ids
    }

    /// Mutably borrows the plan identifier generator for test or composition setup.
    pub fn plan_ids_mut(&mut self) -> &mut P {
        &mut self.plan_ids
    }

    /// Plans equip without remote mutation, reusing move selectors and routing.
    pub fn plan_equip(
        &mut self,
        request: &destiny_core::EquipRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::EquipPlan, MutationError>
    where
        R: destiny_core::DefinitionRepository,
    {
        let request = request.as_move_request();
        authorize(&request)?;
        if request.destination.is_vault() {
            return Err(MutationError::InvalidEquipTarget);
        }
        let outcome = self
            .sync
            .sync(ProfileSyncReason::Ordinary, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self.committed_snapshot()?;
        if request.membership != snapshot.metadata.membership_id {
            return Err(MutationError::WrongMembership);
        }
        let item = self.resolve_item(&request)?;
        let destination = self.resolve_plan_destination(&request, &snapshot.characters)?;
        self.validate_equip(&item, &destination, &snapshot)?;
        let ItemLocation::Character(character) = destination else {
            return Err(MutationError::InvalidEquipTarget);
        };
        let expires_at = lifetime.expires_at(now).map_err(MutationError::from)?;
        let plan_id = self.plan_ids.next_plan_id().map_err(MutationError::from)?;
        destiny_core::EquipPlan::new(
            plan_id,
            request.membership,
            item.instance_id,
            item.location,
            character,
            item.equipped,
            outcome.revision(),
            now,
            expires_at,
        )
        .map_err(|_| MutationError::InvalidPlan)
    }

    /// Forces fresh state and repeats class, slot and exotic safety checks.
    /// AlreadySatisfied requires both character location and equipped state.
    pub fn preflight_equip(
        &mut self,
        request: &destiny_core::EquipRequest,
        plan: &destiny_core::EquipPlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<destiny_core::EquipPreflight, MutationError>
    where
        R: destiny_core::DefinitionRepository,
    {
        self.fresh_equip_preflight(request, plan, policy, now)
            .map(|(state, _)| state)
    }

    fn fresh_equip_preflight(
        &mut self,
        request: &destiny_core::EquipRequest,
        plan: &destiny_core::EquipPlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<(destiny_core::EquipPreflight, ProfileSyncOutcome), MutationError>
    where
        R: destiny_core::DefinitionRepository,
    {
        let request = request.as_move_request();
        authorize(&request)?;
        let mutation = plan.mutation_plan();
        if mutation.is_expired(now) {
            return Err(MutationError::Expired);
        }
        if request.membership != mutation.membership {
            return Err(MutationError::WrongMembership);
        }
        if request.destination.is_vault() {
            return Err(MutationError::InvalidEquipTarget);
        }
        let (outcome, snapshot, resolved_item, current_source) =
            self.fresh_preflight(mutation, policy, now)?;
        let item = self.resolve_item(&request)?;
        if item.instance_id != mutation.item {
            return Err(MutationError::WrongItem);
        }
        let destination = self.resolve_plan_destination(&request, &snapshot.characters)?;
        if destination != mutation.destination {
            return Err(MutationError::WrongDestination);
        }
        self.validate_equip(&item, &destination, &snapshot)?;
        if plan.already_satisfied(&item) {
            return Ok((destiny_core::EquipPreflight::AlreadySatisfied, outcome));
        }
        self.evaluate_preflight(
            &request,
            mutation,
            &snapshot,
            outcome.revision(),
            resolved_item,
            current_source,
            now,
        )?;
        if plan.is_already_satisfied() {
            // A formerly satisfied plan contains no actions; never call it ready
            // after its equipped-state postcondition stopped holding.
            return Err(MutationError::StaleRevision);
        }
        Ok((destiny_core::EquipPreflight::Ready, outcome))
    }

    fn validate_equip(
        &self,
        item: &ItemInstance,
        destination: &ItemLocation,
        snapshot: &InventorySnapshot,
    ) -> Result<(), MutationError>
    where
        R: destiny_core::DefinitionRepository,
    {
        use destiny_core::domain::ClassType;
        if !matches!(destination, ItemLocation::Character(_)) {
            return Err(MutationError::InvalidEquipTarget);
        }
        if !matches!(
            item.location,
            ItemLocation::Vault | ItemLocation::Character(_)
        ) {
            return Err(MutationError::UnsupportedSource);
        }
        if item.equipped && item.location != *destination {
            return Err(MutationError::EquippedOnOtherCharacter);
        }
        let definition = self.equip_definition(item.item_hash)?;
        let slot = definition
            .slot
            .filter(|slot| equipment_category(*slot).is_some())
            .ok_or(MutationError::NotEquippable)?;
        match definition.class_type {
            Some(ClassType::Any) => {}
            Some(ClassType::Unknown) | None => return Err(MutationError::UnknownClass),
            Some(required) => {
                let class = snapshot
                    .characters
                    .iter()
                    .find(|character| ItemLocation::Character(character.id.clone()) == *destination)
                    .and_then(|character| character.class.as_deref())
                    .and_then(|class| class.parse::<ClassType>().ok())
                    .filter(|class| {
                        matches!(
                            class,
                            ClassType::Titan | ClassType::Hunter | ClassType::Warlock
                        )
                    })
                    .ok_or(MutationError::UnknownClass)?;
                if class != required {
                    return Err(MutationError::WrongClass);
                }
            }
        }
        let tier = definition
            .tier
            .as_deref()
            .filter(|tier| !tier.trim().is_empty())
            .ok_or(MutationError::UnknownRarity)?;
        if tier == "Exotic" {
            for existing in snapshot.items.iter().filter(|existing| {
                existing.equipped
                    && existing.location == *destination
                    && existing.instance_id != item.instance_id
            }) {
                let other = match self
                    .repository()
                    .lookup_definition(existing.item_hash)
                    .map_err(MutationError::from)?
                {
                    Resolution::Unique(definition) => definition,
                    // Without its definition we cannot establish that this
                    // equipped neighbor is outside the exotic-limit category.
                    Resolution::Missing => return Err(MutationError::UnknownRarity),
                    Resolution::Ambiguous(_) => return Err(MutationError::InvalidProjection),
                };
                let Some(other_slot) = exotic_neighbor_slot(&other)? else {
                    continue;
                };
                if other_slot == slot || equipment_category(other_slot) != equipment_category(slot)
                {
                    // Same-slot replacement and independent categories do not
                    // require the neighbor's rarity to establish compatibility.
                    continue;
                }
                let other_tier = other
                    .tier
                    .as_deref()
                    .filter(|tier| !tier.trim().is_empty())
                    .ok_or(MutationError::UnknownRarity)?;
                if other_tier == "Exotic" {
                    return Err(MutationError::ExoticConflict);
                }
            }
        }
        Ok(())
    }

    /// Exact catalog lookup avoids truncating a definition search for safety.
    fn equip_definition(
        &self,
        hash: destiny_core::ItemHash,
    ) -> Result<destiny_core::ItemDefinition, MutationError>
    where
        R: destiny_core::DefinitionRepository,
    {
        match self
            .repository()
            .lookup_definition(hash)
            .map_err(MutationError::from)?
        {
            Resolution::Unique(definition) => Ok(definition),
            Resolution::Missing => Err(MutationError::NotEquippable),
            Resolution::Ambiguous(_) => Err(MutationError::InvalidProjection),
        }
    }

    /// DMP-003: authorizes intent, resolves the request, and builds one plan.
    ///
    /// Explicit intent is validated before any side effect, the ordinary
    /// freshness gate runs next, and the item and destination are resolved
    /// against the committed projection without guessing. Nothing here calls a
    /// provider mutation endpoint: the only seams touched are the profile
    /// gateway through the sync service and the read-only repository.
    pub fn plan_move(
        &mut self,
        request: &MoveRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        authorize(request)?;
        let outcome = self
            .sync
            .sync(ProfileSyncReason::Ordinary, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self.committed_snapshot()?;
        if request.membership != snapshot.metadata.membership_id {
            return Err(MutationError::WrongMembership);
        }
        let item = self.resolve_item(request)?;
        let destination = self.resolve_plan_destination(request, &snapshot.characters)?;
        self.build_plan(
            request,
            &item,
            destination,
            outcome.revision(),
            lifetime,
            now,
        )
    }

    /// DMP-004: re-checks one plan against a mandatory fresh preflight.
    ///
    /// Intent is validated again before the refresh, then
    /// [`ProfileSyncReason::Preflight`] forces one synchronized projection
    /// regardless of freshness. The stable item identity, the destination, the
    /// current source, the membership, and the committed revision are
    /// re-resolved, and the shared core policy is evaluated. `Ok(())` means the
    /// plan survived preflight; every non-valid policy outcome is a specific
    /// typed error.
    ///
    /// The request's own selectors are then resolved against the same refreshed
    /// projection and must match the plan's item and destination uniquely, so a
    /// caller cannot carry intent for one move and preflight a plan for another.
    pub fn preflight_move(
        &mut self,
        request: &MoveRequest,
        plan: &MovePlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<(), MutationError> {
        authorize(request)?;
        if request.membership != plan.membership {
            return Err(MutationError::WrongMembership);
        }
        if plan.requires_equipped() {
            return Err(MutationError::EquipNotYetSupported);
        }
        let (outcome, snapshot, resolved_item, current_source) =
            self.fresh_preflight(plan, policy, now)?;
        self.evaluate_preflight(
            request,
            plan,
            &snapshot,
            outcome.revision(),
            resolved_item,
            current_source,
            now,
        )
    }

    /// The single shared adapter entry: plan, then pass the mandatory preflight.
    ///
    /// Both freshness gates run: planning uses the ordinary gate (a fresh
    /// projection is not re-fetched) and preflight always refreshes. A
    /// presentation adapter that calls this cannot obtain a plan that skipped
    /// the preflight policy.
    pub fn prepare_move(
        &mut self,
        request: &MoveRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        let plan = self.plan_move(request, lifetime, policy, now)?;
        self.preflight_move(request, &plan, policy, now)?;
        Ok(plan)
    }

    /// DTE-003: the mandatory fresh preflight gate for one execution attempt.
    ///
    /// Intent is validated first, before any side effect. Expiry and membership
    /// are known from the plan and the request alone, so both are rejected
    /// *before* the forced refresh: an expired or wrong-membership plan must not
    /// reach the provider at all. Only a plan that is still executable and bound
    /// to the requesting membership forces the same
    /// [`ProfileSyncReason::Preflight`] acquisition and shared policy evaluation
    /// [`MutationService::preflight_move`] uses, so execution cannot reach a
    /// changed plan and this gate does not duplicate that policy. The core
    /// ticket checks re-verify expiry, revision, and membership from the
    /// refreshed observation after the refresh.
    ///
    /// The refreshed projection is inspected before the binding policy, because
    /// an already-satisfied destination is the one outcome the strict policy
    /// would otherwise misreport as a stale or changed plan. When the stable
    /// plan item re-resolves uniquely *and* its freshly observed location is
    /// already the plan's destination, the requested end state holds: the gate
    /// returns [`ExecutionAuthorization::VerifiedNoOp`] with zero transfer calls
    /// and no ticket, because no provider mutation may be issued. The request's
    /// own item/destination selectors are still enforced on that path.
    ///
    /// Otherwise the plan must survive the full preflight policy, and the gate
    /// mints the single-use [`ExecutionTicket`] from that same refreshed,
    /// revision-bound observation.
    pub fn authorize_execution(
        &mut self,
        request: &MoveRequest,
        plan: &MovePlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ExecutionAuthorization, MutationError> {
        authorize(request)?;
        if plan.requires_equipped() {
            return Err(MutationError::EquipNotYetSupported);
        }
        // Expiry and membership need no provider read, so they are rejected
        // here, before the forced refresh, and a caller cannot spend a provider
        // call on a plan that could never execute.
        if plan.is_expired(now) {
            return Err(MutationError::Expired);
        }
        if request.membership != plan.membership {
            return Err(MutationError::WrongMembership);
        }
        let (outcome, snapshot, resolved_item, current_source) =
            self.fresh_preflight(plan, policy, now)?;

        if matches!(&resolved_item, Resolution::Unique(_))
            && plan.already_satisfied(&current_source)
        {
            // The concrete plan item re-resolved uniquely and already sits at
            // the plan's destination. The request must still refer to that same
            // plan, so a request for another item or destination is rejected
            // instead of silently reporting someone else's already-satisfied
            // state as a no-op.
            let requested_item = self.resolve_item(request)?;
            if requested_item.instance_id != plan.item {
                return Err(MutationError::WrongItem);
            }
            let requested_destination =
                self.resolve_plan_destination(request, &snapshot.characters)?;
            if requested_destination != plan.destination {
                return Err(MutationError::WrongDestination);
            }
            return Ok(ExecutionAuthorization::VerifiedNoOp(
                ExecutionEvidence::for_plan(plan),
            ));
        }

        self.evaluate_preflight(
            request,
            plan,
            &snapshot,
            outcome.revision(),
            resolved_item,
            current_source,
            now,
        )?;
        let ticket = ExecutionTicket::after_fresh_preflight(plan, &outcome, now)
            .map_err(MutationError::from)?;
        Ok(ExecutionAuthorization::Authorized(ticket))
    }

    /// DTE-003: the narrow executor entry point that consumes one ticket.
    ///
    /// The [`ExecutionTicket`] is taken **by value** and consumed for the whole
    /// sequence, so a caller cannot borrow, copy, or replay the one fresh
    /// preflight that authorized this attempt. Only the plan's ordered steps are
    /// issued, through the supplied [`TransferGateway`] one step at a time, and
    /// the loop stops at the first step that does not complete: a later step's
    /// precondition is the previous step's effect, so continuing would act on an
    /// unproven state.
    ///
    /// The returned [`ExecutionEvidence`] is bounded by the plan and sanitized.
    /// Success returns the evidence directly, and any failure returns an
    /// [`ExecutionFailure`] that still carries the evidence observed so far, so a
    /// later pre-dispatch failure reports the earlier steps that already applied
    /// instead of dropping them. That failure shape distinguishes a first-step
    /// pre-dispatch failure, whose evidence has no attempted step and therefore
    /// represents no remote mutation, from a failure after a provider effect,
    /// which the DTE-004 orchestration refreshes before deciding anything. A
    /// post-dispatch outcome whose effect cannot be determined is recorded as
    /// [`StepOutcome::Uncertain`], never collapsed into a success or a clean
    /// failure. This narrow seam never refreshes or verifies the final
    /// destination on its own; [`MutationService::execute_move_verified`] is the
    /// composed DTE-004 path that does, and this executor deliberately stays
    /// unchanged so one fresh preflight remains bound to one ordered attempt.
    pub fn execute_move<T: TransferGateway>(
        &mut self,
        transfer: &mut T,
        plan: &MovePlan,
        ticket: ExecutionTicket,
    ) -> Result<ExecutionEvidence, ExecutionFailure> {
        // The bounded skeleton is built first and mutated in place, so any
        // failure below can report exactly how much of the plan was attempted
        // without losing the record of an earlier applied step.
        let mut evidence = ExecutionEvidence::for_plan(plan);
        match self.run_steps(transfer, None, plan, &ticket, &mut evidence) {
            Ok(()) => Ok(evidence),
            Err(error) => Err(ExecutionFailure::new(error, evidence)),
        }
    }

    /// Dispatches one freshly authorized plan and classifies acknowledgement.
    ///
    /// The historical name and timestamp arguments remain for compatibility.
    /// `post_action_now` must not precede preflight, but no post-action read is
    /// issued. Dispatched attempts invalidate repository freshness; complete
    /// receipts yield `Applied`. Incomplete/uncertain evidence never claims
    /// observed state or rollback. A zero-attempt pre-dispatch error remains Err.
    pub fn execute_move_verified<T: TransferGateway>(
        &mut self,
        transfer: &mut T,
        request: &MoveRequest,
        plan: &MovePlan,
        policy: FreshnessPolicy,
        preflight_now: Timestamp,
        post_action_now: Timestamp,
    ) -> Result<MoveExecutionOutcome, ExecutionFailure> {
        // The post-action observation cannot precede the attempt it observes, so
        // the caller's clock input is validated before any side effect.
        if post_action_now < preflight_now {
            return Err(ExecutionFailure::new(
                MutationError::InvalidTimestamps,
                ExecutionEvidence::for_plan(plan),
            ));
        }
        // The mandatory fresh preflight, unchanged from DTE-003: expiry or a
        // membership mismatch is rejected before any provider read, and an
        // already-satisfied destination is a verified no-op with no dispatch.
        let authorization = match self.authorize_execution(request, plan, policy, preflight_now) {
            Ok(authorization) => authorization,
            Err(error) => {
                // The attempt was never dispatched, so the bounded skeleton is
                // the whole evidence and no post-action refresh is warranted.
                return Err(ExecutionFailure::new(
                    error,
                    ExecutionEvidence::for_plan(plan),
                ));
            }
        };
        let ticket = match authorization {
            ExecutionAuthorization::VerifiedNoOp(evidence) => {
                return Ok(MoveExecutionOutcome::VerifiedNoOp(evidence));
            }
            ExecutionAuthorization::Authorized(ticket) => ticket,
        };

        let revision = ticket.revision();
        let (evidence, attempt_error) = match self.execute_move(transfer, plan, ticket) {
            Ok(evidence) => (evidence, None),
            Err(failure) => {
                if !failure.requires_post_action_refresh() {
                    // No step reached the provider, so no remote mutation
                    // occurred and no post-action refresh may be spent.
                    return Err(failure);
                }
                // At least one step reached the provider. The evidence observed
                // so far must survive into the classification, and the
                // pre-dispatch error is retained for an honest report.
                let ExecutionFailure { error, evidence } = failure;
                (*evidence, Some(error))
            }
        };

        let _ = revision;
        Ok(self.acknowledge_attempt(plan, evidence, attempt_error))
    }

    /// Executes an equip intent with acknowledgement-based (optimistic) success.
    ///
    /// The gateway pair is (transfer, equip). A single fresh preflight either
    /// proves a no-op or authorizes one ordered attempt. The outcome is Applied
    /// only when every planned step is acknowledged; the profile is not re-read,
    /// and local freshness is invalidated so the next read syncs. No retry occurs.
    pub fn execute_equip_verified<T: TransferGateway, E: destiny_core::EquipGateway>(
        &mut self,
        gateways: (&mut T, &mut E),
        request: &destiny_core::EquipRequest,
        plan: &destiny_core::EquipPlan,
        policy: FreshnessPolicy,
        preflight_now: Timestamp,
        post_action_now: Timestamp,
    ) -> Result<MoveExecutionOutcome, ExecutionFailure>
    where
        R: destiny_core::DefinitionRepository,
    {
        let mutation = plan.mutation_plan();
        let failure = |error| ExecutionFailure::new(error, ExecutionEvidence::for_plan(mutation));
        if post_action_now < preflight_now {
            return Err(failure(MutationError::InvalidTimestamps));
        }
        let (state, observation) = self
            .fresh_equip_preflight(request, plan, policy, preflight_now)
            .map_err(failure)?;
        if state == destiny_core::EquipPreflight::AlreadySatisfied {
            return Ok(MoveExecutionOutcome::VerifiedNoOp(
                ExecutionEvidence::for_plan(mutation),
            ));
        }
        let ticket = ExecutionTicket::after_fresh_preflight(mutation, &observation, preflight_now)
            .map_err(|error| failure(MutationError::from(error)))?;
        let revision = ticket.revision();
        let (evidence, attempt_error) = match self.execute_equip(gateways, plan, ticket) {
            Ok(evidence) => (evidence, None),
            Err(failure) => {
                if !failure.requires_post_action_refresh() {
                    return Err(failure);
                }
                (*failure.evidence, Some(failure.error))
            }
        };
        let _ = revision;
        Ok(self.acknowledge_attempt(mutation, evidence, attempt_error))
    }

    /// Consumes one fresh ticket and dispatches transfers before equip, without retries.
    pub fn execute_equip<T: TransferGateway, E: destiny_core::EquipGateway>(
        &mut self,
        gateways: (&mut T, &mut E),
        plan: &destiny_core::EquipPlan,
        ticket: ExecutionTicket,
    ) -> Result<ExecutionEvidence, ExecutionFailure> {
        let mut evidence = ExecutionEvidence::for_plan(plan.mutation_plan());
        match self.run_steps(
            gateways.0,
            Some(gateways.1),
            plan.mutation_plan(),
            &ticket,
            &mut evidence,
        ) {
            Ok(()) => Ok(evidence),
            Err(error) => Err(ExecutionFailure::new(error, evidence)),
        }
    }

    /// Classifies receipts without a post-action provider request.
    fn acknowledge_attempt(
        &mut self,
        plan: &MovePlan,
        evidence: ExecutionEvidence,
        attempt_error: Option<MutationError>,
    ) -> MoveExecutionOutcome {
        // Even refusals and uncertain attempts invalidate the pre-action cache.
        if let Err(error) = self.repository_mut().invalidate_freshness() {
            return MoveExecutionOutcome::VerificationFailed(VerificationFailure::new(
                VerificationFailureReason::RefreshFailed,
                None,
                Some(MutationError::from(error)),
                evidence,
            ));
        }
        if evidence.completed_count() == plan.steps().len()
            && evidence
                .steps()
                .iter()
                .all(|step| step.outcome == StepOutcome::Completed)
            && attempt_error.is_none()
        {
            MoveExecutionOutcome::Applied(evidence)
        } else {
            MoveExecutionOutcome::IncompleteExecution(IncompleteExecution::new(
                attempt_error,
                expected_location(plan, evidence.completed_count()),
                evidence,
            ))
        }
    }

    /// Runs the ticket's ordered steps in place, stopping at the first that does
    /// not apply.
    ///
    /// `evidence` is mutated before any failure can return, so the caller wraps
    /// the error with exactly the steps observed so far. The complete membership
    /// identity, the definition hash, and the accepted quantity are read from the
    /// committed projection rather than caller input, so an adapter cannot be
    /// handed an invented request shape.
    fn run_steps<T: TransferGateway>(
        &mut self,
        transfer: &mut T,
        mut equip: Option<&mut dyn destiny_core::EquipGateway>,
        plan: &MovePlan,
        ticket: &ExecutionTicket,
        evidence: &mut ExecutionEvidence,
    ) -> Result<(), MutationError> {
        // The ticket is the authorization and the plan supplies the bounded
        // evidence skeleton, so the two must describe the same authorized move.
        // A mismatch can only come from a contract violation, and it fails closed
        // before any provider call.
        if equip.is_none() && plan.requires_equipped() {
            // A move-only composition cannot execute an equip intent.
            return Err(MutationError::InvalidPlan);
        }
        if ticket.plan_id() != &plan.plan_id
            || ticket.item() != &plan.item
            || ticket.steps() != plan.steps()
        {
            return Err(MutationError::InvalidPlan);
        }
        let snapshot = self.committed_snapshot()?;
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id.clone(),
        )
        .map_err(MutationError::from)?;
        if membership.membership_id() != ticket.membership() {
            return Err(MutationError::WrongMembership);
        }
        // The ticket only authorizes the safety checks performed on this exact
        // revision. Production must additionally serialize local publication
        // across preflight and the whole ordered attempt; a comparison alone
        // cannot close the check-to-dispatch race.
        if snapshot.metadata.revision != ticket.revision() {
            return Err(MutationError::StaleRevision);
        }
        let instance = match self
            .repository()
            .lookup_instance(ticket.item())
            .map_err(MutationError::from)?
        {
            Resolution::Unique(instance) => instance,
            Resolution::Missing => return Err(MutationError::MissingResolution),
            Resolution::Ambiguous(_) => return Err(MutationError::AmbiguousResolution),
        };
        for step in ticket.steps() {
            let outcome = match &step.action {
                StepAction::Transfer { from, to } => {
                    let request = TransferRequest::new(
                        membership.clone(),
                        ticket.item().clone(),
                        instance.item_hash,
                        instance.quantity,
                        from.clone(),
                        to.clone(),
                    )
                    .map_err(MutationError::from)?;
                    let outcome = transfer.transfer(&request).map_err(MutationError::from)?;
                    StepOutcome::from_transfer(outcome)
                }
                StepAction::Equip { character } => {
                    let request = destiny_core::EquipItemRequest::new(
                        membership.clone(),
                        ticket.item().clone(),
                        character.clone(),
                    )
                    .map_err(MutationError::from)?;
                    let gateway = equip.as_deref_mut().ok_or(MutationError::InvalidPlan)?;
                    StepOutcome::from_equip(gateway.equip(&request).map_err(MutationError::from)?)
                }
            };
            evidence
                .record(step.ordinal, outcome)
                .map_err(MutationError::from)?;
            if outcome == StepOutcome::AuthenticationRefused {
                // A received auth refusal is attempted evidence, unlike expired
                // credentials or a transport failure known to precede dispatch.
                return Err(MutationError::from(AppError::new(
                    ErrorCategory::ProviderAuthentication,
                )));
            }
            if !outcome.continues() {
                break;
            }
        }
        Ok(())
    }

    /// Acquires the mandatory fresh preflight observation exactly once.
    ///
    /// [`ProfileSyncReason::Preflight`] forces one synchronized projection
    /// regardless of freshness, then the plan's stable item identity and its
    /// current location are re-resolved against that same projection. Both the
    /// preflight policy and the execution gate consume this one acquisition, so
    /// neither can drift into a second, differently-sourced read.
    fn fresh_preflight(
        &mut self,
        plan: &MovePlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<
        (
            ProfileSyncOutcome,
            InventorySnapshot,
            Resolution<ItemInstanceId>,
            ItemLocation,
        ),
        MutationError,
    > {
        let outcome = self
            .sync
            .sync(ProfileSyncReason::Preflight, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self.committed_snapshot()?;
        // Bind the refreshed account before resolving identities or allowing
        // either equip or move's already-satisfied shortcut.
        if plan.membership != snapshot.metadata.membership_id {
            return Err(MutationError::WrongMembership);
        }
        let (resolved_item, current_source) = self.reresolve_item(plan)?;
        Ok((outcome, snapshot, resolved_item, current_source))
    }

    /// Evaluates the shared core policy and then the request-selector re-check.
    ///
    /// The plan-anchored evaluation proves the plan is still internally
    /// consistent; the request-anchored resolution proves the caller's intent
    /// refers to that same plan. Both selectors are resolved against the same
    /// refreshed projection and must match the plan uniquely, so a request for
    /// one item can never carry a plan for another. This is the exact policy
    /// [`MutationService::preflight_move`] applies, shared rather than
    /// duplicated.
    #[allow(clippy::too_many_arguments)]
    fn evaluate_preflight(
        &self,
        request: &MoveRequest,
        plan: &MovePlan,
        snapshot: &InventorySnapshot,
        current_revision: SyncRevision,
        resolved_item: Resolution<ItemInstanceId>,
        current_source: ItemLocation,
        now: Timestamp,
    ) -> Result<(), MutationError> {
        let resolved_destination = match &plan.destination {
            ItemLocation::Vault => Resolution::Unique(ItemLocation::Vault),
            ItemLocation::Character(id) => resolve_destination(
                &DestinationSelector::Character(id.clone()),
                &snapshot.characters,
            ),
            // An unsupported planned destination is rejected by the shared core
            // policy before the re-resolved destination is consulted.
            ItemLocation::Postmaster(_) | ItemLocation::Other(_) => Resolution::Missing,
        };
        let input = MutationPolicyInput {
            now,
            membership: request.membership.clone(),
            current_revision,
            current_source,
            resolved_item,
            resolved_destination,
        };
        match evaluate_mutation(plan, &input) {
            MutationPolicyOutcome::Valid => {}
            outcome => return Err(MutationError::from_policy_outcome(outcome)),
        }
        let requested_item = self.resolve_item(request)?;
        if requested_item.instance_id != plan.item {
            return Err(MutationError::WrongItem);
        }
        let requested_destination = self.resolve_plan_destination(request, &snapshot.characters)?;
        if requested_destination != plan.destination {
            return Err(MutationError::WrongDestination);
        }
        Ok(())
    }

    /// Returns the committed projection, or reports that none is visible.
    pub(crate) fn committed_snapshot(&self) -> Result<InventorySnapshot, MutationError> {
        self.repository()
            .load_current()
            .map_err(MutationError::from)?
            .ok_or(MutationError::InvalidProjection)
    }

    /// Resolves the requested item to exactly one concrete instance.
    pub(crate) fn resolve_item(
        &self,
        request: &MoveRequest,
    ) -> Result<ItemInstance, MutationError> {
        match &request.item {
            ItemSelector::Instance(instance_id) => {
                match self
                    .repository()
                    .lookup_instance(instance_id)
                    .map_err(MutationError::from)?
                {
                    Resolution::Unique(instance) => Ok(instance),
                    Resolution::Missing => Err(MutationError::MissingItem),
                    Resolution::Ambiguous(_) => Err(MutationError::AmbiguousItem),
                }
            }
            ItemSelector::Semantic(term) => {
                let definitions = self
                    .repository()
                    .search_definitions(&DefinitionSearchQuery {
                        tier: None,
                        text: Some(term.clone()),
                        item_type: None,
                        tags: Vec::new(),
                        element: None,
                        slot: None,
                        class_type: None,
                        limit: MAX_SEARCH_LIMIT,
                    })
                    .map_err(MutationError::from)?;
                if definitions.is_empty() {
                    return Err(MutationError::MissingItem);
                }
                // A term that matches more definitions than the bounded
                // instance filter can carry is ambiguous, never truncated. The
                // term itself never appears in the error.
                if definitions.len() > MAX_HASH_FILTERS {
                    return Err(MutationError::AmbiguousItem);
                }
                let instances = self
                    .repository()
                    .search_instances(&InstanceSearchQuery {
                        sort: None,
                        location: None,
                        equipped: None,
                        include_available: false,
                        item_hashes: ItemHashFilter::AnyOf(
                            definitions
                                .iter()
                                .map(|definition| definition.hash)
                                .collect(),
                        ),
                        perk_hash: None,
                        perk_name: None,
                        mod_name: None,
                        archetype: None,
                        stats: Vec::new(),
                        character_id: None,
                        definitions: None,
                        min_power: None,
                        limit: MAX_SEARCH_LIMIT,
                    })
                    .map_err(MutationError::from)?;
                match instances.as_slice() {
                    [] => Err(MutationError::MissingItem),
                    [instance] => Ok(instance.clone()),
                    _ => Err(MutationError::AmbiguousItem),
                }
            }
        }
    }

    /// Resolves the requested destination to exactly one supported location.
    fn resolve_plan_destination(
        &self,
        request: &MoveRequest,
        characters: &[destiny_core::Character],
    ) -> Result<ItemLocation, MutationError> {
        match resolve_destination(&request.destination, characters) {
            Resolution::Unique(location) => Ok(location),
            Resolution::Missing => Err(MutationError::MissingDestination),
            Resolution::Ambiguous(_) => Err(MutationError::AmbiguousDestination),
        }
    }

    /// Re-resolves the plan's stable item identity and its current location.
    ///
    /// A unique resolution gives both the identity and the current source. When
    /// the item does not resolve, the plan's own source is submitted so the
    /// shared policy reports the resolution failure rather than a misleading
    /// source mismatch.
    fn reresolve_item(
        &self,
        plan: &MovePlan,
    ) -> Result<(Resolution<destiny_core::ItemInstanceId>, ItemLocation), MutationError> {
        match self
            .repository()
            .lookup_instance(&plan.item)
            .map_err(MutationError::from)?
        {
            Resolution::Unique(instance) => {
                let source = instance.location.clone();
                Ok((Resolution::Unique(instance.instance_id), source))
            }
            Resolution::Missing => Ok((Resolution::Missing, plan.source.clone())),
            Resolution::Ambiguous(instances) => Ok((
                Resolution::Ambiguous(
                    instances
                        .into_iter()
                        .map(|instance| instance.instance_id)
                        .collect(),
                ),
                plan.source.clone(),
            )),
        }
    }

    /// Builds one bounded, revision-bound plan through the injected id seam.
    fn build_plan(
        &mut self,
        request: &MoveRequest,
        item: &ItemInstance,
        destination: ItemLocation,
        revision: destiny_core::SyncRevision,
        lifetime: PlanLifetime,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        // The bounded expiry is computed before a plan identifier is spent.
        let expires_at = lifetime.expires_at(now).map_err(MutationError::from)?;
        let plan_id = self.plan_ids.next_plan_id().map_err(MutationError::from)?;
        MovePlan::new(
            plan_id,
            request.membership.clone(),
            item.instance_id.clone(),
            item.location.clone(),
            destination,
            revision,
            now,
            expires_at,
        )
        .map_err(|validation| match validation {
            MovePlanValidation::SameLocation => MutationError::SameLocation,
            MovePlanValidation::UnsupportedLocation => MutationError::UnsupportedLocation,
            MovePlanValidation::InvalidTimestamps | MovePlanValidation::LifetimeExceeded => {
                MutationError::InvalidPlan
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EquipmentCategory {
    Weapon,
    Armor,
}

/// Weapon and armor exotic limits are independent; unsupported slots are not
/// treated as equipment. Slots come from normalized Manifest bucket hashes.
fn equipment_category(slot: destiny_core::domain::ItemSlot) -> Option<EquipmentCategory> {
    use destiny_core::domain::ItemSlot;
    match slot {
        ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power => Some(EquipmentCategory::Weapon),
        ItemSlot::Helmet
        | ItemSlot::Gauntlets
        | ItemSlot::Chest
        | ItemSlot::Legs
        | ItemSlot::ClassItem => Some(EquipmentCategory::Armor),
        ItemSlot::Other | ItemSlot::Unknown => None,
    }
}

/// Other is explicitly non-equipment. The Manifest normalizer leaves `slot`
/// absent for every bucket outside these eight weapon/armor hashes; a present
/// bucket outside that exact set therefore also proves category irrelevance.
/// Missing/Unknown slot without that proof is not a candidate-equippability
/// failure: it is an unknown neighbor classification and fails closed.
fn exotic_neighbor_slot(
    definition: &destiny_core::ItemDefinition,
) -> Result<Option<destiny_core::domain::ItemSlot>, MutationError> {
    use destiny_core::domain::ItemSlot;
    match definition.slot {
        Some(ItemSlot::Other) => Ok(None),
        Some(slot) if equipment_category(slot).is_some() => Ok(Some(slot)),
        _ => {
            if definition
                .bucket_hash
                .is_some_and(|bucket| ItemSlot::from_equipment_bucket(bucket).is_none())
            {
                Ok(None)
            } else {
                Err(MutationError::UnknownNeighborSlot)
            }
        }
    }
}

/// Rejects absent or ambiguous explicit intent before any side effect.
pub(crate) fn authorize(request: &MoveRequest) -> Result<(), MutationError> {
    match request.authorization() {
        IntentAuthorizationOutcome::Authorized => Ok(()),
        IntentAuthorizationOutcome::MissingIntent => Err(MutationError::MissingIntent),
        IntentAuthorizationOutcome::AmbiguousIntent => Err(MutationError::AmbiguousIntent),
    }
}

/// The location a sequence with `completed` applied steps is expected to leave
/// the item at.
///
/// This is how the post-action verification tells an honest incomplete execution
/// from a contradiction: with no applied step the item must still be at the
/// plan's source, and with `n` applied steps it must be at the destination of
/// step `n`.
/// A sequence can never report more applied steps than the plan has, so the
/// fallback is unreachable and only keeps the helper total.
fn expected_location(plan: &MovePlan, completed: usize) -> ItemLocation {
    if completed == 0 {
        return plan.source.clone();
    }
    plan.steps()
        .get(completed - 1)
        .map(|step| match &step.action {
            StepAction::Transfer { to, .. } => to.clone(),
            StepAction::Equip { character } => ItemLocation::Character(character.clone()),
        })
        .unwrap_or_else(|| plan.destination.clone())
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::{
        ExecutionAuthorization, ExecutionFailure, MoveExecutionOutcome, MutationError,
        MutationService, VerificationFailureReason,
    };
    use crate::sync::ProfileSyncService;
    use crate::test_support::{
        DeterministicPlanIds, InMemoryInventoryRepository, InMemorySearchRepository,
        QueuedProfileGateway, QueuedTransferGateway,
    };
    use destiny_core::{
        AppError, Character, CharacterId, DefinitionSearchQuery, DefinitionVersion,
        DestinationSelector, ErrorCategory, ExecutionEvidence, ExecutionTicket, FreshnessPolicy,
        InstanceSearchQuery, InventoryRepository, InventorySnapshot, InventorySnapshotInput,
        ItemDefinition, ItemHash, ItemInstance, ItemInstanceId, ItemLocation, ItemSelector,
        MAX_HASH_FILTERS, MoveIntentAuthorization, MovePlan, MoveRequest, MutationPolicyOutcome,
        PlanId, PlanIdGenerator, PlanLifetime, ProfileGateway, Resolution, SearchRepository,
        SourceMarker, StepAction, StepOutcome, SyncRevision, Timestamp, TransferOutcome,
        TransferRejection,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(50_000);
    const POST_ACTION_NOW: Timestamp = Timestamp::new(50_010);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    const LIFETIME: PlanLifetime = PlanLifetime::MAX;
    const DEFINITION_VERSION: &str = "definitions-v1";
    const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

    /// A narrow test-local composite exposing one repository for both seams.
    ///
    /// Committing a profile refresh also mirrors its instances into the search
    /// projection, so a plan resolved after a refresh observes the new
    /// projection. Every read-seam call is recorded so tests can prove the exact
    /// sync-then-resolve ordering and that no mutation seam exists.
    #[derive(Clone, Debug, Default)]
    struct CompositeRepository {
        inventory: RefCell<InMemoryInventoryRepository>,
        publication_on_lookup: RefCell<Option<InventorySnapshotInput>>,
        search: InMemorySearchRepository,
        catalog: crate::test_support::InMemoryDefinitionRepository,
        hides_current: bool,
        fails_invalidation: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            Self {
                inventory: RefCell::new(InMemoryInventoryRepository::new()),
                publication_on_lookup: RefCell::new(None),
                search: InMemorySearchRepository::with_definitions(definitions.clone()),
                catalog: crate::test_support::InMemoryDefinitionRepository::new(
                    DefinitionVersion::new(DEFINITION_VERSION),
                    definitions,
                ),
                hides_current: false,
                fails_invalidation: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        /// Hides every committed projection to model an unavailable local store.
        fn hiding_current() -> Self {
            Self {
                hides_current: true,
                ..Self::default()
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        fn recorded_calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }

        /// Commits one valid projection, mirrors its instances, and reports the revision.
        fn commit(&mut self, input: InventorySnapshotInput) -> SyncRevision {
            let snapshot = self
                .inventory
                .get_mut()
                .replace_current(input)
                .expect("valid seed snapshot");
            self.search.set_instances(snapshot.items.clone());
            snapshot.metadata.revision
        }

        /// Seeds the search projection with structural duplicates.
        ///
        /// A structurally valid projection can never carry duplicate instance
        /// identifiers, so this models the contract-violating state the search
        /// port documents as the only route to an ambiguous exact lookup.
        fn seed_instances(&mut self, instances: Vec<ItemInstance>) {
            self.search.set_instances(instances);
        }
    }

    impl InventoryRepository for CompositeRepository {
        fn freshness_invalidated(&self) -> Result<bool, AppError> {
            self.inventory.borrow().freshness_invalidated()
        }
        fn invalidate_freshness(&mut self) -> Result<(), AppError> {
            if self.fails_invalidation {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            self.inventory.get_mut().invalidate_freshness()
        }
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.record("load_current");
            if self.hides_current {
                return Ok(None);
            }
            self.inventory.borrow().load_current()
        }

        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.record("replace_current");
            let snapshot = self.inventory.get_mut().replace_current(input)?;
            self.search.set_instances(snapshot.items.clone());
            Ok(snapshot)
        }
    }

    impl destiny_core::DefinitionRepository for CompositeRepository {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.catalog.current_definition_version()
        }

        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<ItemDefinition>, AppError> {
            self.catalog.lookup_definition(hash)
        }

        fn current_metadata(&self) -> Result<Option<destiny_core::SyncMetadata>, AppError> {
            self.catalog.current_metadata()
        }

        fn replace_definitions(
            &mut self,
            replacement: destiny_core::DefinitionReplacementInput,
        ) -> Result<destiny_core::SyncMetadata, AppError> {
            self.catalog.replace_definitions(replacement)
        }
    }

    impl SearchRepository for CompositeRepository {
        fn search_definitions(
            &self,
            query: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.record("search_definitions");
            self.search.search_definitions(query)
        }

        fn search_instances(
            &self,
            query: &InstanceSearchQuery,
        ) -> Result<Vec<ItemInstance>, AppError> {
            self.record("search_instances");
            self.search.search_instances(query)
        }

        fn lookup_instance(
            &self,
            instance_id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.record("lookup_instance");
            if let Some(input) = self.publication_on_lookup.borrow_mut().take() {
                // Model another process publishing after refresh, before lookup.
                let snapshot = self.inventory.borrow_mut().replace_current(input)?;
                return Ok(snapshot
                    .items
                    .into_iter()
                    .find(|item| &item.instance_id == instance_id)
                    .map_or(Resolution::Missing, Resolution::Unique));
            }
            self.search.lookup_instance(instance_id)
        }
    }

    /// A read-only spy over the fake gateway the sync service owns by value.
    struct SharedGateway(Rc<RefCell<QueuedProfileGateway>>);

    impl ProfileGateway for SharedGateway {
        fn resolve_membership(&mut self) -> Result<destiny_core::DestinyMembership, AppError> {
            self.0.borrow_mut().resolve_membership()
        }

        fn load_inventory(
            &mut self,
            membership: &destiny_core::DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.borrow_mut().load_inventory(membership)
        }
    }

    /// A plan identifier seam that reports exhaustion immediately.
    #[derive(Debug, Default)]
    struct ExhaustedPlanIds;

    impl PlanIdGenerator for ExhaustedPlanIds {
        fn next_plan_id(&mut self) -> Result<PlanId, AppError> {
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        }
    }

    type Service = MutationService<SharedGateway, CompositeRepository, DeterministicPlanIds>;

    fn equip_definition(hash: i64, slot: destiny_core::domain::ItemSlot) -> ItemDefinition {
        let mut value = definition(hash, "Equip candidate");
        value.slot = Some(slot);
        value.class_type = Some(destiny_core::domain::ClassType::Any);
        value.tier = Some("Exotic".into());
        value
    }

    fn equip_request() -> destiny_core::EquipRequest {
        destiny_core::EquipRequest {
            membership: membership_id(),
            item: ItemSelector::instance(item_id("candidate")),
            destination: DestinationSelector::character(CharacterId::new("warlock")),
            intent: MoveIntentAuthorization::Explicit,
        }
    }

    #[test]
    fn rc001_direct_move_preflight_rejects_membership_before_io() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .unwrap();
        service.repository().calls.borrow_mut().clear();
        queue_refresh(&shared, fresh_vault_projection());
        let mut wrong_request = move_request.clone();
        wrong_request.membership = destiny_core::MembershipId::new("4611686018484639999");

        assert_eq!(
            service.preflight_move(&wrong_request, &plan, TTL, NOW),
            Err(MutationError::WrongMembership)
        );
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert!(service.repository().recorded_calls().is_empty());

        // The refused call must leave the queued refresh available for a valid
        // same-account preflight, which still performs its mandatory refresh.
        assert_eq!(
            service.preflight_move(&move_request, &plan, TTL, NOW),
            Ok(())
        );
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn rc001_planning_binds_projection_membership() {
        for equip in [true, false] {
            let shared = gateway();
            let mut repo = CompositeRepository::with_definitions(vec![equip_definition(
                1,
                destiny_core::domain::ItemSlot::Kinetic,
            )]);
            repo.commit(projection(
                NOW,
                vec![warlock_character()],
                vec![instance("candidate", 1, ItemLocation::Vault)],
            ));
            let mut service = service(&shared, repo);
            let mut req = equip_request();
            req.membership = destiny_core::MembershipId::new("4611686018484639999");
            let error = if equip {
                service.plan_equip(&req, LIFETIME, TTL, NOW).unwrap_err()
            } else {
                service
                    .plan_move(&req.as_move_request(), LIFETIME, TTL, NOW)
                    .unwrap_err()
            };
            assert_eq!(error, MutationError::WrongMembership);
        }
    }

    #[test]
    fn rc001_refreshed_account_cannot_authorize_noop_or_dispatch() {
        for equip in [true, false] {
            for on_target in [true, false] {
                for wrong_account in [false, true] {
                    let shared = gateway();
                    let mut repo = CompositeRepository::with_definitions(vec![equip_definition(
                        1,
                        destiny_core::domain::ItemSlot::Kinetic,
                    )]);
                    repo.commit(projection(
                        NOW,
                        vec![warlock_character()],
                        vec![instance("candidate", 1, ItemLocation::Vault)],
                    ));
                    let mut service = service(&shared, repo);
                    let req = equip_request();
                    let equip_plan = if equip {
                        Some(service.plan_equip(&req, LIFETIME, TTL, NOW).unwrap())
                    } else {
                        None
                    };
                    let move_plan = if !equip {
                        Some(
                            service
                                .plan_move(&req.as_move_request(), LIFETIME, TTL, NOW)
                                .unwrap(),
                        )
                    } else {
                        None
                    };
                    let mut item = instance(
                        "candidate",
                        1,
                        if on_target {
                            warlock()
                        } else {
                            ItemLocation::Vault
                        },
                    );
                    item.equipped = on_target;
                    let mut refreshed = projection(NOW, vec![warlock_character()], vec![item]);
                    if wrong_account {
                        refreshed.membership_id =
                            destiny_core::MembershipId::new("4611686018484639999");
                    }
                    let membership = destiny_core::DestinyMembership::new(
                        refreshed.membership_type,
                        refreshed.membership_id.clone(),
                    )
                    .unwrap();
                    shared.borrow_mut().queue_membership(Ok(membership));
                    shared.borrow_mut().queue_inventory(Ok(refreshed));
                    let mut transfer = QueuedTransferGateway::new();
                    let mut equip_gateway = crate::test_support::QueuedEquipGateway::new();
                    // Same-account dispatch cases are covered by existing execution tests.
                    if !wrong_account && !on_target {
                        continue;
                    }
                    let result = if let Some(plan) = equip_plan {
                        service.execute_equip_verified(
                            (&mut transfer, &mut equip_gateway),
                            &req,
                            &plan,
                            TTL,
                            NOW,
                            POST_ACTION_NOW,
                        )
                    } else {
                        service.execute_move_verified(
                            &mut transfer,
                            &req.as_move_request(),
                            move_plan.as_ref().unwrap(),
                            TTL,
                            NOW,
                            POST_ACTION_NOW,
                        )
                    };
                    if wrong_account {
                        assert_eq!(result.unwrap_err().error(), MutationError::WrongMembership);
                    } else {
                        assert!(matches!(
                            result.unwrap(),
                            MoveExecutionOutcome::VerifiedNoOp(_)
                        ));
                    }
                    assert!(transfer.requests().is_empty());
                    assert!(equip_gateway.requests().is_empty());
                }
            }
        }
    }

    #[test]
    fn equip_transfer_routes_and_equipped_noop() {
        use destiny_core::MutationStep;
        for (source, equipped, count) in [
            (ItemLocation::Vault, false, 2),
            (hunter(), false, 3),
            (warlock(), false, 1),
            (warlock(), true, 0),
        ] {
            let shared = gateway();
            let mut repo = CompositeRepository::with_definitions(vec![equip_definition(
                1,
                destiny_core::domain::ItemSlot::Kinetic,
            )]);
            let mut item = instance("candidate", 1, source.clone());
            item.equipped = equipped;
            repo.commit(projection(
                NOW,
                vec![
                    warlock_character(),
                    character("hunter", None, Some("Hunter")),
                ],
                vec![item],
            ));
            let plan = service(&shared, repo)
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap();
            assert_eq!(plan.steps().len(), count);
            assert_eq!(plan.is_already_satisfied(), equipped);
            assert_eq!(plan.via_vault(), count == 3);
            let mut expected = match source {
                ItemLocation::Vault => {
                    vec![MutationStep::transfer(1, ItemLocation::Vault, warlock())]
                }
                source if source == hunter() => vec![
                    MutationStep::transfer(1, hunter(), ItemLocation::Vault),
                    MutationStep::transfer(2, ItemLocation::Vault, warlock()),
                ],
                source if source == warlock() => Vec::new(),
                _ => panic!("unexpected test source"),
            };
            if !equipped {
                expected.push(MutationStep {
                    ordinal: count as u8,
                    action: StepAction::Equip {
                        character: CharacterId::new("warlock"),
                    },
                });
            }
            assert_eq!(plan.steps(), expected);
        }
    }

    #[test]
    fn equip_refuses_equipped_elsewhere_wrong_class_and_nonequipment() {
        use destiny_core::domain::{ClassType, ItemSlot};
        for (equipped, class, slot, expected) in [
            (
                true,
                ClassType::Any,
                Some(ItemSlot::Helmet),
                MutationError::EquippedOnOtherCharacter,
            ),
            (
                false,
                ClassType::Titan,
                Some(ItemSlot::Helmet),
                MutationError::WrongClass,
            ),
            (
                false,
                ClassType::Unknown,
                Some(ItemSlot::Helmet),
                MutationError::UnknownClass,
            ),
            (false, ClassType::Any, None, MutationError::NotEquippable),
            (
                false,
                ClassType::Any,
                Some(ItemSlot::Other),
                MutationError::NotEquippable,
            ),
        ] {
            let shared = gateway();
            let mut def = equip_definition(1, ItemSlot::Helmet);
            def.slot = slot;
            def.class_type = Some(class);
            let mut repo = CompositeRepository::with_definitions(vec![def]);
            let mut item = instance("candidate", 1, hunter());
            item.equipped = equipped;
            repo.commit(projection(
                NOW,
                vec![
                    character("warlock", None, Some("Warlock")),
                    character("hunter", None, Some("Hunter")),
                ],
                vec![item],
            ));
            assert_eq!(
                service(&shared, repo).plan_equip(&equip_request(), LIFETIME, TTL, NOW),
                Err(expected)
            );
            assert_eq!(shared.borrow().inventory_calls(), 0);
        }
    }

    #[test]
    fn equip_exotic_conflicts_only_across_slots_in_same_category() {
        use destiny_core::domain::ItemSlot;
        for (candidate, existing, refused) in [
            (ItemSlot::Kinetic, ItemSlot::Energy, true),
            (ItemSlot::Kinetic, ItemSlot::Kinetic, false),
            (ItemSlot::Helmet, ItemSlot::Legs, true),
            (ItemSlot::Helmet, ItemSlot::Helmet, false),
            (ItemSlot::Helmet, ItemSlot::Energy, false),
        ] {
            let shared = gateway();
            let mut repo = CompositeRepository::with_definitions(vec![
                equip_definition(1, candidate),
                equip_definition(2, existing),
            ]);
            let mut other = instance("other", 2, warlock());
            other.equipped = true;
            repo.commit(projection(
                NOW,
                vec![warlock_character()],
                vec![instance("candidate", 1, ItemLocation::Vault), other],
            ));
            let result = service(&shared, repo).plan_equip(&equip_request(), LIFETIME, TTL, NOW);
            if refused {
                assert_eq!(result, Err(MutationError::ExoticConflict));
            } else {
                assert!(result.is_ok());
            }
        }
    }

    #[test]
    fn equip_rejects_missing_ambiguous_and_vault_selectors() {
        use destiny_core::domain::ItemSlot;
        for (selector, target, expected) in [
            (
                ItemSelector::instance(item_id("missing")),
                equip_request().destination,
                MutationError::MissingItem,
            ),
            (
                ItemSelector::semantic("Equip candidate").unwrap(),
                equip_request().destination,
                MutationError::AmbiguousItem,
            ),
            (
                equip_request().item,
                DestinationSelector::Vault,
                MutationError::InvalidEquipTarget,
            ),
        ] {
            let shared = gateway();
            let mut repo =
                CompositeRepository::with_definitions(vec![equip_definition(1, ItemSlot::Kinetic)]);
            repo.commit(projection(
                NOW,
                vec![warlock_character()],
                vec![
                    instance("candidate", 1, ItemLocation::Vault),
                    instance("duplicate", 1, ItemLocation::Vault),
                ],
            ));
            let mut request = equip_request();
            request.item = selector;
            request.destination = target;
            assert_eq!(
                service(&shared, repo).plan_equip(&request, LIFETIME, TTL, NOW),
                Err(expected)
            );
        }
    }

    #[test]
    fn equip_preflight_rechecks_flags_location_and_exotic_conflict() {
        use destiny_core::domain::ItemSlot;
        for change in 0..5 {
            let shared = gateway();
            let mut repo = CompositeRepository::with_definitions(vec![
                equip_definition(1, ItemSlot::Kinetic),
                equip_definition(2, ItemSlot::Energy),
            ]);
            let chars = vec![
                warlock_character(),
                character("hunter", None, Some("Hunter")),
            ];
            repo.commit(projection(
                NOW,
                chars.clone(),
                vec![instance("candidate", 1, ItemLocation::Vault)],
            ));
            let mut svc = service(&shared, repo);
            let plan = svc
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap();
            let mut item = instance("candidate", 1, ItemLocation::Vault);
            let mut items = Vec::new();
            match change {
                0 => {
                    item.location = hunter();
                    item.equipped = true;
                }
                1 => {
                    item.location = warlock();
                    item.equipped = true;
                }
                2 => {
                    let mut other = instance("other", 2, warlock());
                    other.equipped = true;
                    items.push(other);
                }
                3 => item.location = hunter(),
                _ => {}
            }
            items.push(item);
            shared.borrow_mut().queue_membership(Ok(membership()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(projection(NOW, chars, items)));
            let result = svc.preflight_equip(&equip_request(), &plan, TTL, NOW);
            match change {
                0 => assert_eq!(result, Err(MutationError::EquippedOnOtherCharacter)),
                1 => assert_eq!(result, Ok(destiny_core::EquipPreflight::AlreadySatisfied)),
                2 => assert_eq!(result, Err(MutationError::ExoticConflict)),
                3 => assert_eq!(result, Err(MutationError::StaleRevision)),
                _ => assert_eq!(result, Ok(destiny_core::EquipPreflight::Ready)),
            }
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }
    }

    #[test]
    fn equip_verified_execution_scenarios() {
        use crate::test_support::QueuedEquipGateway;
        use destiny_core::{EquipOutcome, EquipRejection, domain::ItemSlot};
        // 0-2: routes; 3: no-op; 4-5: equip refusals; 6-8: uncertainty;
        // 9: transfer refusal; 10: elsewhere; 11: stale preflight;
        // 12: transient; 13: pre-dispatch equip error; 14: equipped contradiction;
        // 15: newly satisfied preflight; 16: failed post-action refresh;
        // 17/22: stale confirmed transfer/equip; 18: stale uncertain equip;
        // 19: stale auth refusal; 20/21: unchanged partial plans.
        // Each case uses realistic equipped non-equipment neighbors.
        for case in 0..23 {
            let shared = gateway();
            let source = match case {
                0 | 3 | 18..=20 | 22 => warlock(),
                2 => hunter(),
                _ => ItemLocation::Vault,
            };
            let mut item = instance("candidate", 1, source.clone());
            item.equipped = case == 3;
            let mut neighbors = Vec::new();
            let mut definitions = vec![equip_definition(1, ItemSlot::Kinetic)];
            for (hash, name) in [(2, "Subclass"), (3, "Ghost"), (4, "Emblem")] {
                let mut def = definition(hash, name);
                def.slot = Some(ItemSlot::Other);
                definitions.push(def);
                let mut neighbor = instance(&format!("neighbor-{hash}"), hash, warlock());
                neighbor.equipped = true;
                neighbors.push(neighbor);
            }
            let chars = vec![
                warlock_character(),
                character("hunter", None, Some("Hunter")),
            ];
            let mut before = neighbors.clone();
            before.push(item.clone());
            let input = projection(NOW, chars.clone(), before);
            let mut repo = CompositeRepository::with_definitions(definitions);
            repo.commit(input);
            let mut svc = service(&shared, repo);
            let plan = svc
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap();
            shared.borrow_mut().queue_membership(Ok(membership()));
            if case == 11 {
                item.location = hunter();
            }
            if case == 15 {
                item.location = warlock();
                item.equipped = true;
            }
            let mut preflight = neighbors.clone();
            preflight.push(item.clone());
            shared
                .borrow_mut()
                .queue_inventory(Ok(projection(NOW, chars.clone(), preflight)));
            let mut transfer = QueuedTransferGateway::new();
            for _ in 0..2 {
                transfer.queue(Ok(if matches!(case, 9 | 21) {
                    TransferOutcome::Rejected(TransferRejection::DestinationFull)
                } else {
                    TransferOutcome::Transferred
                }));
            }
            let mut equip = QueuedEquipGateway::new();
            let outcome = match case {
                4 => EquipOutcome::Rejected(EquipRejection::ExoticLimit),
                5 => EquipOutcome::Rejected(EquipRejection::NotInSocialSpace),
                6..=8 | 18 => EquipOutcome::Uncertain,
                19 => EquipOutcome::AuthenticationRefused,
                20 => EquipOutcome::Rejected(EquipRejection::ExoticLimit),
                12 => EquipOutcome::Transient,
                _ => EquipOutcome::Equipped,
            };
            equip.queue(if case == 13 {
                Err(AppError::new(ErrorCategory::ProviderUnavailable))
            } else {
                Ok(outcome)
            });
            item.location = match case {
                8 | 10 => hunter(),
                17..=22 => source.clone(),
                9 => source.clone(),
                _ => warlock(),
            };
            item.equipped = matches!(case, 0..=3 | 6);
            neighbors.push(item);
            if !matches!(case, 3 | 11 | 15) {
                shared.borrow_mut().queue_membership(Ok(membership()));
                shared.borrow_mut().queue_inventory(if case == 16 {
                    Err(AppError::new(ErrorCategory::ProviderUnavailable))
                } else {
                    Ok(projection(POST_ACTION_NOW, chars, neighbors))
                });
            }
            let result = svc.execute_equip_verified(
                (&mut transfer, &mut equip),
                &equip_request(),
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            );
            if case == 11 {
                assert_eq!(result.unwrap_err().error(), MutationError::StaleRevision);
                assert_eq!((transfer.calls(), equip.calls()), (0, 0));
                continue;
            }
            let result = result.unwrap();
            let evidence = match result {
                MoveExecutionOutcome::VerifiedNoOp(evidence) => {
                    assert!(matches!(case, 3 | 15));
                    evidence
                }
                MoveExecutionOutcome::Applied(evidence) => {
                    assert!(matches!(case, 0..=2 | 10 | 14 | 16 | 17 | 22));
                    evidence
                }
                MoveExecutionOutcome::VerifiedSuccess(_) => panic!("no post-action verification"),
                MoveExecutionOutcome::IncompleteExecution(incomplete) => {
                    assert!(matches!(case, 4..=9 | 12 | 13 | 18..=21));
                    incomplete.evidence().clone()
                }
                MoveExecutionOutcome::AppliedPendingProviderRefresh(_) => {
                    panic!("unexpected pending outcome")
                }
                MoveExecutionOutcome::VerificationFailed(failure) => {
                    assert!(matches!(case, 8 | 10 | 14 | 16));
                    assert_eq!(
                        failure.reason(),
                        if case == 16 {
                            VerificationFailureReason::RefreshFailed
                        } else {
                            VerificationFailureReason::Elsewhere
                        }
                    );
                    failure.evidence().clone()
                }
            };
            assert_eq!(shared.borrow().inventory_calls(), 1);
            assert_eq!(
                equip.calls(),
                if matches!(case, 3 | 9 | 15 | 21) {
                    0
                } else {
                    1
                }
            );
            assert_eq!(
                transfer.calls(),
                match case {
                    0 | 3 | 15 | 18..=20 | 22 => 0,
                    2 => 2,
                    _ => 1,
                }
            );
            if matches!(case, 0..=2 | 14 | 16) {
                assert_eq!(evidence.completed_count(), plan.steps().len());
            }
            if matches!(case, 4 | 5 | 7 | 12 | 13) {
                assert_eq!(evidence.completed_count(), 1);
                assert!(evidence.steps()[0].outcome.continues());
            }
            if case == 12 {
                assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Transient));
            }
            if matches!(case, 6..=8) {
                assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Uncertain));
            }
            if case == 4 || case == 5 {
                assert_eq!(
                    evidence.stop_outcome(),
                    Some(StepOutcome::EquipRejected(if case == 4 {
                        EquipRejection::ExoticLimit
                    } else {
                        EquipRejection::NotInSocialSpace
                    }))
                );
            }
            if equip.calls() == 1 {
                assert_eq!(equip.requests()[0].item(), &plan.mutation_plan().item);
                assert_eq!(
                    equip.requests()[0].character(),
                    &CharacterId::new("warlock")
                );
            }
        }
    }

    #[test]
    fn equip_move_entry_point_fails_closed_before_any_transfer() {
        use destiny_core::domain::ItemSlot;
        for (source, equipped) in [
            (ItemLocation::Vault, false),
            (hunter(), false),
            (warlock(), false),
            (warlock(), true),
        ] {
            let shared = gateway();
            let mut repo =
                CompositeRepository::with_definitions(vec![equip_definition(1, ItemSlot::Kinetic)]);
            let mut item = instance("candidate", 1, source);
            item.equipped = equipped;
            let input = projection(
                NOW,
                vec![warlock_character(), character("hunter", None, None)],
                vec![item],
            );
            repo.commit(input.clone());
            let mut svc = service(&shared, repo);
            let plan = svc
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap();
            assert!(!plan.mutation_plan().already_satisfied(&warlock()));
            assert_eq!(
                svc.authorize_execution(
                    &equip_request().as_move_request(),
                    plan.mutation_plan(),
                    TTL,
                    NOW,
                ),
                Err(MutationError::EquipNotYetSupported)
            );
            assert_eq!(shared.borrow().inventory_calls(), 0);
            shared.borrow_mut().queue_membership(Ok(membership()));
            shared.borrow_mut().queue_inventory(Ok(input));
            let outcome = svc
                .sync_mut()
                .sync(destiny_core::ProfileSyncReason::Preflight, TTL, NOW)
                .unwrap();
            let ticket =
                ExecutionTicket::after_fresh_preflight(plan.mutation_plan(), &outcome, NOW)
                    .unwrap();
            let mut transfer = QueuedTransferGateway::new();
            let failure = svc
                .execute_move(&mut transfer, plan.mutation_plan(), ticket)
                .unwrap_err();
            assert_eq!(failure.error(), MutationError::InvalidPlan);
            assert_eq!(transfer.calls(), 0);
            assert!(!failure.requires_post_action_refresh());
        }
    }

    #[test]
    fn equip_class_compatibility_uses_stored_restrictions_and_target_class() {
        use destiny_core::domain::{ClassType, ItemSlot};
        for (restriction, target_class, expected) in [
            (Some(ClassType::Warlock), Some("wArLoCk"), None),
            (
                Some(ClassType::Warlock),
                None,
                Some(MutationError::UnknownClass),
            ),
            (None, Some("Warlock"), Some(MutationError::UnknownClass)),
            (Some(ClassType::Any), None, None),
        ] {
            let shared = gateway();
            let mut def = equip_definition(1, ItemSlot::Helmet);
            def.class_type = restriction;
            let mut repo = CompositeRepository::with_definitions(vec![def]);
            repo.commit(projection(
                NOW,
                vec![character("warlock", None, target_class)],
                vec![instance("candidate", 1, ItemLocation::Vault)],
            ));
            let result = service(&shared, repo).plan_equip(&equip_request(), LIFETIME, TTL, NOW);
            assert_eq!(result.err(), expected);
        }
    }

    #[test]
    fn equip_selectors_and_intent_reuse_move_rules_without_spending_plan_ids() {
        use destiny_core::domain::ItemSlot;
        let shared = gateway();
        let mut repo =
            CompositeRepository::with_definitions(vec![equip_definition(1, ItemSlot::Kinetic)]);
        repo.commit(projection(
            NOW,
            vec![character("warlock", None, Some("Warlock"))],
            vec![instance("candidate", 1, ItemLocation::Vault)],
        ));
        let mut svc = service(&shared, repo);
        let mut request = equip_request();
        for (intent, expected) in [
            (
                MoveIntentAuthorization::Absent,
                MutationError::MissingIntent,
            ),
            (
                MoveIntentAuthorization::Ambiguous,
                MutationError::AmbiguousIntent,
            ),
        ] {
            request.intent = intent;
            assert_eq!(svc.plan_equip(&request, LIFETIME, TTL, NOW), Err(expected));
        }
        assert!(svc.repository().recorded_calls().is_empty());
        assert_eq!(shared.borrow().membership_calls(), 0);
        request.intent = MoveIntentAuthorization::Explicit;
        request.item = ItemSelector::semantic("Equip candidate").unwrap();
        request.destination = DestinationSelector::character_label("WARLOCK").unwrap();
        let plan = svc.plan_equip(&request, LIFETIME, TTL, NOW).unwrap();
        assert_eq!(plan.mutation_plan().item, item_id("candidate"));
        assert_eq!(plan.mutation_plan().destination, warlock());
        assert_eq!(svc.plan_ids().calls(), 1);
    }

    #[test]
    fn equip_preflight_invalidates_a_formerly_equipped_noop() {
        use destiny_core::domain::ItemSlot;
        let shared = gateway();
        let mut repo =
            CompositeRepository::with_definitions(vec![equip_definition(1, ItemSlot::Kinetic)]);
        let mut item = instance("candidate", 1, warlock());
        item.equipped = true;
        repo.commit(projection(
            NOW,
            vec![warlock_character()],
            vec![item.clone()],
        ));
        let mut svc = service(&shared, repo);
        let plan = svc
            .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
            .unwrap();
        assert!(plan.is_already_satisfied());
        item.equipped = false;
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared.borrow_mut().queue_inventory(Ok(projection(
            NOW,
            vec![warlock_character()],
            vec![item],
        )));
        assert_eq!(
            svc.preflight_equip(&equip_request(), &plan, TTL, NOW),
            Err(MutationError::StaleRevision)
        );
    }

    #[test]
    fn equip_refuses_missing_candidate_rarity() {
        use destiny_core::domain::ItemSlot;
        let shared = gateway();
        let mut def = equip_definition(1, ItemSlot::Kinetic);
        def.tier = None;
        let mut repo = CompositeRepository::with_definitions(vec![def]);
        repo.commit(projection(
            NOW,
            vec![warlock_character()],
            vec![instance("candidate", 1, ItemLocation::Vault)],
        ));
        let mut svc = service(&shared, repo);
        assert_eq!(
            svc.plan_equip(&equip_request(), LIFETIME, TTL, NOW),
            Err(MutationError::UnknownRarity)
        );
        assert_eq!(svc.plan_ids().calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn equip_refuses_unknown_neighbor_rarity_only_when_safety_depends_on_it() {
        use destiny_core::domain::ItemSlot;
        for (candidate_tier, neighbor_slot, missing_definition, expected) in [
            (
                "Exotic",
                ItemSlot::Energy,
                false,
                Some(MutationError::UnknownRarity),
            ),
            (
                "Exotic",
                ItemSlot::Energy,
                true,
                Some(MutationError::UnknownRarity),
            ),
            ("Exotic", ItemSlot::Kinetic, false, None),
            ("Exotic", ItemSlot::Helmet, false, None),
            ("Legendary", ItemSlot::Energy, false, None),
        ] {
            let shared = gateway();
            let mut candidate = equip_definition(1, ItemSlot::Kinetic);
            candidate.tier = Some(candidate_tier.into());
            let mut neighbor = equip_definition(2, neighbor_slot);
            neighbor.tier = None;
            let definitions = if missing_definition {
                vec![candidate]
            } else {
                vec![candidate, neighbor]
            };
            let mut repo = CompositeRepository::with_definitions(definitions);
            let mut other = instance("neighbor", 2, warlock());
            other.equipped = true;
            repo.commit(projection(
                NOW,
                vec![warlock_character()],
                vec![instance("candidate", 1, ItemLocation::Vault), other],
            ));
            let mut svc = service(&shared, repo);
            let result = svc.plan_equip(&equip_request(), LIFETIME, TTL, NOW);
            assert_eq!(result.err(), expected);
            assert_eq!(shared.borrow().inventory_calls(), 0);
        }
    }

    #[test]
    fn equip_preflight_rechecks_changed_rarity_definitions_and_class_restrictions() {
        use destiny_core::domain::{ClassType, ItemSlot};
        for change in 0..4 {
            let shared = gateway();
            let candidate = equip_definition(1, ItemSlot::Kinetic);
            let mut neighbor = equip_definition(2, ItemSlot::Energy);
            neighbor.tier = Some("Legendary".into());
            let mut repo =
                CompositeRepository::with_definitions(vec![candidate.clone(), neighbor.clone()]);
            let mut other = instance("neighbor", 2, warlock());
            other.equipped = true;
            let input = projection(
                NOW,
                vec![character("warlock", None, Some("Warlock"))],
                vec![instance("candidate", 1, ItemLocation::Vault), other],
            );
            repo.commit(input.clone());
            let mut svc = service(&shared, repo);
            let plan = svc
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap();
            let mut candidate = candidate;
            let definitions = match change {
                0 => {
                    candidate.tier = None;
                    vec![candidate, neighbor]
                }
                1 => {
                    neighbor.tier = None;
                    vec![candidate, neighbor]
                }
                2 => vec![candidate],
                _ => {
                    candidate.class_type = Some(ClassType::Titan);
                    vec![candidate, neighbor]
                }
            };
            svc.repository_mut().catalog = crate::test_support::InMemoryDefinitionRepository::new(
                DefinitionVersion::new(DEFINITION_VERSION),
                definitions,
            );
            shared.borrow_mut().queue_membership(Ok(membership()));
            shared.borrow_mut().queue_inventory(Ok(input));
            assert_eq!(
                svc.preflight_equip(&equip_request(), &plan, TTL, NOW),
                Err(if change == 3 {
                    MutationError::WrongClass
                } else {
                    MutationError::UnknownRarity
                })
            );
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }
    }

    #[test]
    fn equip_exotic_ignores_realistic_nonequipment_neighbors() {
        use destiny_core::domain::ItemSlot;
        for exotic_neighbor in [false, true] {
            let shared = gateway();
            let mut weapon = equip_definition(2, ItemSlot::Energy);
            weapon.tier = Some(
                if exotic_neighbor {
                    "Exotic"
                } else {
                    "Legendary"
                }
                .into(),
            );
            let mut definitions = vec![equip_definition(1, ItemSlot::Kinetic), weapon];
            let mut items = vec![instance("candidate", 1, ItemLocation::Vault)];
            for (id, hash, name) in [
                ("weapon", 2, "Weapon"),
                ("subclass", 3, "Subclass"),
                ("ghost", 4, "Ghost"),
                ("emblem", 5, "Emblem"),
            ] {
                if hash != 2 {
                    let mut def = definition(hash, name);
                    def.slot = Some(ItemSlot::Other);
                    definitions.push(def);
                }
                let mut item = instance(id, hash, warlock());
                item.equipped = true;
                items.push(item);
            }
            let mut repo = CompositeRepository::with_definitions(definitions);
            let input = projection(NOW, vec![warlock_character()], items);
            repo.commit(input.clone());
            let mut svc = service(&shared, repo);
            let result = svc.plan_equip(&equip_request(), LIFETIME, TTL, NOW);
            if exotic_neighbor {
                assert_eq!(result, Err(MutationError::ExoticConflict));
            } else {
                let plan = result.unwrap();
                shared.borrow_mut().queue_membership(Ok(membership()));
                shared.borrow_mut().queue_inventory(Ok(input));
                assert_eq!(
                    svc.preflight_equip(&equip_request(), &plan, TTL, NOW),
                    Ok(destiny_core::EquipPreflight::Ready)
                );
            }
        }
    }

    #[test]
    fn equip_unknown_neighbor_slot_requires_a_nonequipment_bucket() {
        use destiny_core::domain::ItemSlot;
        for slot in [None, Some(ItemSlot::Unknown)] {
            for (bucket, expected) in [
                (Some(destiny_core::SUBCLASS_BUCKET_HASH), None),
                (None, Some(MutationError::UnknownNeighborSlot)),
                (
                    Some(destiny_core::BucketHash::new(1498876634)),
                    Some(MutationError::UnknownNeighborSlot),
                ),
            ] {
                let shared = gateway();
                let mut neighbor = definition(2, "Unknown neighbor");
                neighbor.slot = slot;
                neighbor.bucket_hash = bucket;
                let mut repo = CompositeRepository::with_definitions(vec![
                    equip_definition(1, ItemSlot::Kinetic),
                    neighbor,
                ]);
                let mut other = instance("neighbor", 2, warlock());
                other.equipped = true;
                repo.commit(projection(
                    NOW,
                    vec![warlock_character()],
                    vec![instance("candidate", 1, ItemLocation::Vault), other],
                ));
                assert_eq!(
                    service(&shared, repo)
                        .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                        .err(),
                    expected
                );
            }
        }
    }

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
    }

    fn lock_request() -> destiny_core::LockRequest {
        destiny_core::LockRequest {
            membership: membership_id(),
            item: exact("item-1"),
            locked: true,
            intent: MoveIntentAuthorization::Explicit,
        }
    }

    #[test]
    fn lock_plan_resolution_and_refusals() {
        let shared = gateway();
        let mut repo = CompositeRepository::with_definitions(vec![definition(1, "Lock candidate")]);
        repo.commit(fresh_vault_projection());
        let mut svc = service(&shared, repo);
        let mut req = lock_request();
        let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
        assert_eq!(plan.item(), &item_id("item-1"));
        assert_eq!(plan.character(), &CharacterId::new("warlock"));
        req.item = semantic("Lock candidate");
        assert!(svc.plan_lock(&req, LIFETIME, TTL, NOW).is_ok());
        req.item = exact("missing");
        assert_eq!(
            svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap_err(),
            MutationError::MissingItem
        );
        req.item = exact("item-1");
        req.intent = MoveIntentAuthorization::Absent;
        assert_eq!(
            svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap_err(),
            MutationError::MissingIntent
        );
        req.intent = MoveIntentAuthorization::Explicit;
        svc.repository_mut().seed_instances(vec![
            instance("item-1", 1, ItemLocation::Vault),
            instance("second", 1, ItemLocation::Vault),
        ]);
        req.item = semantic("Lock candidate");
        assert_eq!(
            svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap_err(),
            MutationError::AmbiguousItem
        );
        svc.repository_mut()
            .seed_instances(vec![instance("", 1, ItemLocation::Vault)]);
        req.item = exact("");
        assert!(svc.plan_lock(&req, LIFETIME, TTL, NOW).is_err());
        svc.repository_mut().seed_instances(vec![instance(
            "item-1",
            1,
            ItemLocation::Other("unknown".into()),
        )]);
        req.item = exact("item-1");
        assert_eq!(
            svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap_err(),
            MutationError::UnsupportedSource
        );
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn postmaster_lock_refuses_owned_source_with_zero_dispatch() {
        use crate::test_support::QueuedLockGateway;
        let shared = gateway();
        let mut repo = CompositeRepository::with_definitions(vec![definition(1, "Candidate")]);
        repo.commit(fresh_vault_projection());
        let mut svc = service(&shared, repo);
        let req = lock_request();
        let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
        let mut refreshed = fresh_vault_projection();
        refreshed.items[0].location = ItemLocation::Postmaster(CharacterId::new("warlock"));
        queue_refresh(&shared, refreshed);
        let mut lock = QueuedLockGateway::new();
        let failure = svc
            .execute_lock_verified(&mut lock, &req, &plan, TTL, NOW, POST_ACTION_NOW)
            .unwrap_err();
        assert_eq!(failure.error(), MutationError::UnsupportedSource);
        assert_eq!(failure.evidence().attempt(), None);
        assert_eq!(lock.calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(
            svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap_err(),
            MutationError::UnsupportedSource
        );
    }

    #[test]
    fn postmaster_equip_refuses_owned_source() {
        use destiny_core::domain::ItemSlot;
        let shared = gateway();
        let mut repo =
            CompositeRepository::with_definitions(vec![equip_definition(1, ItemSlot::Kinetic)]);
        repo.commit(projection(
            NOW,
            vec![warlock_character()],
            vec![instance(
                "candidate",
                1,
                ItemLocation::Postmaster(CharacterId::new("warlock")),
            )],
        ));
        assert_eq!(
            service(&shared, repo)
                .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                .unwrap_err(),
            MutationError::UnsupportedSource
        );
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn lock_optimistic_execution_scenarios() {
        use crate::lock::LockExecutionOutcome;
        use crate::test_support::QueuedLockGateway;
        use destiny_core::{LockOutcome, LockRejection};
        // Acknowledgement, fresh no-op, rejection, uncertainty, provider lag,
        // unused post-action failures/drift, pre-dispatch errors, auth and unlock.
        for case in 0..15 {
            let shared = gateway();
            let chars = vec![warlock_character(), character("hunter", None, None)];
            let mut item = instance("item-1", 1, ItemLocation::Vault);
            item.locked = case == 13;
            let mut repo = CompositeRepository::with_definitions(vec![]);
            repo.commit(projection(NOW, chars.clone(), vec![item.clone()]));
            let mut svc = service(&shared, repo);
            let mut req = lock_request();
            req.locked = case != 13;
            let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
            if case == 1 {
                item.locked = true;
            }
            if case == 9 {
                item.location = hunter();
            }
            queue_refresh(
                &shared,
                projection(
                    NOW,
                    chars.clone(),
                    if case == 10 {
                        vec![]
                    } else {
                        vec![item.clone()]
                    },
                ),
            );
            let mut lock = QueuedLockGateway::new();
            let outcome = match case {
                2 => LockOutcome::Rejected(LockRejection::ItemNotFound),
                3 | 4 => LockOutcome::Uncertain,
                11 => LockOutcome::AuthenticationRefused,
                12 => LockOutcome::Transient,
                _ => LockOutcome::Applied,
            };
            lock.queue(if case == 8 {
                Err(AppError::new(ErrorCategory::ProviderUnavailable))
            } else {
                Ok(outcome)
            });
            if !matches!(case, 1 | 8..=10) {
                if matches!(case, 0 | 3 | 13) {
                    item.locked = req.locked;
                }
                if case == 6 {
                    item.quantity = 2; // changed revision, not provider lag
                }
                shared.borrow_mut().queue_membership(Ok(membership()));
                shared.borrow_mut().queue_inventory(if case == 5 {
                    Err(AppError::new(ErrorCategory::ProviderUnavailable))
                } else {
                    Ok(projection(
                        POST_ACTION_NOW,
                        chars,
                        if case == 14 { vec![] } else { vec![item] },
                    ))
                });
            }
            let result =
                svc.execute_lock_verified(&mut lock, &req, &plan, TTL, NOW, POST_ACTION_NOW);
            if matches!(case, 8..=10) {
                let error = result.unwrap_err();
                assert_eq!(error.evidence().attempt(), None);
                assert_eq!(lock.calls(), usize::from(case == 8));
                assert_eq!(shared.borrow().inventory_calls(), 1);
                continue;
            }
            let outcome = result.unwrap();
            match &outcome {
                LockExecutionOutcome::Applied(_) => assert!(matches!(case, 0 | 5..=7 | 13 | 14)),
                LockExecutionOutcome::VerifiedNoOp(_) => assert_eq!(case, 1),
                LockExecutionOutcome::IncompleteExecution(_) => {
                    assert!(matches!(case, 2..=4 | 11 | 12))
                }
                other => panic!("no post-action verification: {other:?}"),
            }
            assert_eq!(
                outcome.evidence().attempt(),
                if case == 1 {
                    None
                } else {
                    Some(outcome_for_case(case))
                }
            );
            assert_eq!(lock.calls(), usize::from(case != 1));
            assert_eq!(shared.borrow().inventory_calls(), 1);
            assert_eq!(svc.repository().freshness_invalidated().unwrap(), case != 1);
            assert_eq!(
                outcome.evidence().observed(),
                if case == 1 { Some(req.locked) } else { None }
            );
            if case != 1 {
                assert_eq!(lock.requests()[0].character(), &CharacterId::new("hunter"));
                assert_eq!(lock.requests()[0].locked(), req.locked);
            }
        }
        fn outcome_for_case(case: usize) -> destiny_core::LockOutcome {
            use destiny_core::{LockOutcome, LockRejection};
            match case {
                2 => LockOutcome::Rejected(LockRejection::ItemNotFound),
                3 | 4 => LockOutcome::Uncertain,
                11 => LockOutcome::AuthenticationRefused,
                12 => LockOutcome::Transient,
                _ => LockOutcome::Applied,
            }
        }
    }

    #[test]
    fn acknowledged_lock_fails_closed_when_local_invalidation_fails() {
        use crate::lock::{LockExecutionOutcome, LockVerificationFailureReason};
        use crate::test_support::QueuedLockGateway;
        let shared = gateway();
        let mut repo = CompositeRepository::with_definitions(vec![]);
        repo.commit(fresh_vault_projection());
        let mut svc = service(&shared, repo);
        let req = lock_request();
        let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
        queue_refresh(&shared, fresh_vault_projection());
        svc.repository_mut().fails_invalidation = true;
        let mut lock = QueuedLockGateway::new();
        lock.queue(Ok(destiny_core::LockOutcome::Applied));
        let outcome = svc
            .execute_lock_verified(&mut lock, &req, &plan, TTL, NOW, POST_ACTION_NOW)
            .unwrap();
        let LockExecutionOutcome::VerificationFailed(failure) = outcome else {
            panic!("storage failure must fail closed: {outcome:?}");
        };
        assert_eq!(
            failure.reason(),
            LockVerificationFailureReason::RefreshFailed
        );
        assert_eq!(
            failure.error(),
            Some(MutationError::Application(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            failure.evidence().attempt(),
            Some(destiny_core::LockOutcome::Applied)
        );
        assert_eq!(failure.evidence().observed(), None);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(lock.calls(), 1);
    }

    #[test]
    fn lock_character_routing_ignores_equip_policy() {
        use crate::test_support::QueuedLockGateway;
        use destiny_core::LockOutcome;
        let shared = gateway();
        let chars = vec![warlock_character(), character("hunter", None, None)];
        let mut item = instance("item-1", 1, warlock());
        item.equipped = true;
        let mut repo = CompositeRepository::with_definitions(vec![]);
        repo.commit(projection(NOW, chars.clone(), vec![item.clone()]));
        let mut svc = service(&shared, repo);
        let req = lock_request();
        let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
        assert_eq!(plan.character(), &CharacterId::new("warlock"));
        queue_refresh(&shared, projection(NOW, chars.clone(), vec![item.clone()]));
        item.locked = true;
        queue_refresh(&shared, projection(POST_ACTION_NOW, chars, vec![item]));
        let mut lock = QueuedLockGateway::new();
        lock.queue(Ok(LockOutcome::Applied));
        assert!(matches!(
            svc.execute_lock_verified(&mut lock, &req, &plan, TTL, NOW, POST_ACTION_NOW),
            Ok(crate::lock::LockExecutionOutcome::Applied(_))
        ));
        assert_eq!(lock.requests()[0].character(), &CharacterId::new("warlock"));
        assert_eq!(lock.calls(), 1);
    }

    #[test]
    fn lock_preflight_binds_intent_plan_time_and_membership_before_io() {
        use crate::test_support::QueuedLockGateway;
        for case in 0..7 {
            let shared = gateway();
            let mut repo = CompositeRepository::with_definitions(vec![]);
            repo.commit(fresh_vault_projection());
            let mut svc = service(&shared, repo);
            let mut req = lock_request();
            let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
            let mut pre = NOW;
            let mut post = POST_ACTION_NOW;
            let expected = match case {
                0 => {
                    req.intent = MoveIntentAuthorization::Absent;
                    MutationError::MissingIntent
                }
                1 => {
                    req.intent = MoveIntentAuthorization::Ambiguous;
                    MutationError::AmbiguousIntent
                }
                2 => {
                    req.membership = destiny_core::MembershipId::new("999");
                    MutationError::WrongMembership
                }
                3 => {
                    req.locked = false;
                    MutationError::InvalidPlan
                }
                4 => {
                    post = Timestamp::new(NOW.seconds() - 1);
                    MutationError::InvalidTimestamps
                }
                5 => {
                    pre = LIFETIME.expires_at(NOW).unwrap();
                    post = pre;
                    MutationError::Expired
                }
                _ => {
                    pre = Timestamp::new(NOW.seconds() - 1);
                    MutationError::Expired
                }
            };
            let mut lock = QueuedLockGateway::new();
            let failure = svc
                .execute_lock_verified(&mut lock, &req, &plan, TTL, pre, post)
                .unwrap_err();
            assert_eq!(failure.error(), expected);
            assert_eq!(failure.evidence().attempt(), None);
            assert_eq!(lock.calls(), 0);
            assert_eq!(shared.borrow().inventory_calls(), 0);
        }
    }

    #[test]
    fn lock_refuses_publication_between_preflight_and_dispatch() {
        use crate::test_support::QueuedLockGateway;
        let shared = gateway();
        let mut repo = CompositeRepository::with_definitions(vec![]);
        repo.commit(fresh_vault_projection());
        let mut svc = service(&shared, repo);
        let req = lock_request();
        let plan = svc.plan_lock(&req, LIFETIME, TTL, NOW).unwrap();
        queue_refresh(&shared, fresh_vault_projection());
        let mut changed = fresh_vault_projection();
        changed.items[0].quantity = 2;
        *svc.repository().publication_on_lookup.borrow_mut() = Some(changed);
        let mut lock = QueuedLockGateway::new();
        let failure = svc
            .execute_lock_verified(&mut lock, &req, &plan, TTL, NOW, POST_ACTION_NOW)
            .unwrap_err();
        assert_eq!(failure.error(), MutationError::StaleRevision);
        assert_eq!(lock.calls(), 0);
    }

    fn service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        repository: CompositeRepository,
    ) -> Service {
        MutationService::new(
            ProfileSyncService::new(SharedGateway(Rc::clone(shared)), repository),
            DeterministicPlanIds::starting_at(1),
        )
    }

    fn membership_id() -> destiny_core::MembershipId {
        destiny_core::MembershipId::new(MEMBERSHIP_IDENTIFIER)
    }

    fn membership() -> destiny_core::DestinyMembership {
        destiny_core::DestinyMembership::new(destiny_core::MembershipType::new(3), membership_id())
            .expect("valid membership")
    }

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

    fn warlock_character() -> Character {
        character("warlock", Some("Warlock"), None)
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(DEFINITION_VERSION),
        }
    }

    fn item_id(value: &str) -> ItemInstanceId {
        ItemInstanceId::new(value)
    }

    fn instance(instance_id: &str, item_hash: i64, location: ItemLocation) -> ItemInstance {
        ItemInstance {
            available_plugs: Vec::new(),
            instance_id: item_id(instance_id),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            stats: Vec::new(),
            plugs: Vec::new(),
            perks: Vec::new(),
        }
    }

    fn warlock() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("warlock"))
    }

    fn hunter() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("hunter"))
    }

    fn projection(
        checked_at: Timestamp,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> InventorySnapshotInput {
        InventorySnapshotInput {
            profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
            stacks: Vec::new(),
            plug_set_entries: Vec::new(),
            membership_id: membership_id(),
            membership_type: destiny_core::MembershipType::new(3),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at,
            source: SourceMarker::Profile,
            characters,
            items,
        }
    }

    fn fresh_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - 100)
    }

    fn stale_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - TTL.ttl_seconds - 1)
    }

    fn queue_refresh(shared: &Rc<RefCell<QueuedProfileGateway>>, input: InventorySnapshotInput) {
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared.borrow_mut().queue_inventory(Ok(input));
    }

    fn request(item: ItemSelector, destination: DestinationSelector) -> MoveRequest {
        MoveRequest {
            membership: membership_id(),
            item,
            destination,
            intent: MoveIntentAuthorization::Explicit,
        }
    }

    fn exact(instance_id: &str) -> ItemSelector {
        ItemSelector::instance(self::item_id(instance_id))
    }

    fn semantic(term: &str) -> ItemSelector {
        ItemSelector::semantic(term).expect("valid semantic term")
    }

    fn exact_character(id: &str) -> DestinationSelector {
        DestinationSelector::character(CharacterId::new(id))
    }

    fn character_label(label: &str) -> DestinationSelector {
        DestinationSelector::character_label(label).expect("valid destination label")
    }

    /// One fresh projection holding a Vault item and one warlock character.
    fn fresh_vault_projection() -> InventorySnapshotInput {
        projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        )
    }

    #[test]
    fn plan_move_builds_one_direct_step_from_the_vault_to_an_exact_character() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        let revision = repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.plan_id, PlanId::new("plan-1"));
        assert_eq!(plan.membership, membership_id());
        assert_eq!(plan.item, item_id("item-1"));
        assert_eq!(plan.source, ItemLocation::Vault);
        assert_eq!(plan.destination, warlock());
        assert_eq!(plan.revision, revision);
        assert_eq!(plan.created_at(), NOW);
        assert_eq!(
            plan.expires_at(),
            Timestamp::new(NOW.seconds() + LIFETIME.ttl_seconds())
        );
        assert_eq!(plan.step_count(), 1);
        assert_eq!(plan.steps()[0].ordinal, 1);
        let StepAction::Transfer { from, to } = &plan.steps()[0].action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &ItemLocation::Vault);
        assert_eq!(to, &warlock());
        assert_eq!(plan.effects().item, item_id("item-1"));
        assert!(!plan.effects().via_vault);
        assert_eq!(service.plan_ids().calls(), 1);
        // The projection was already fresh, so the ordinary gate skipped
        // acquisition entirely and planning stayed a pure read.
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(
            service.repository().recorded_calls(),
            ["load_current", "load_current", "lookup_instance"]
        );
    }

    #[test]
    fn plan_move_resolves_a_semantic_item_through_bounded_search() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.item, item_id("item-1"));
        assert_eq!(plan.source, ItemLocation::Vault);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "search_definitions",
                "search_instances"
            ]
        );
    }

    #[test]
    fn plan_move_rejects_a_semantic_item_that_matches_nothing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("thorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing item");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_a_semantic_item_that_matches_two_instances() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 1, warlock()),
            ],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous item");

        assert_eq!(error, MutationError::AmbiguousItem);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_treats_a_term_matching_too_many_definitions_as_ambiguous() {
        let shared = gateway();
        let definitions: Vec<ItemDefinition> = (0..(MAX_HASH_FILTERS + 1))
            .map(|index| definition(index as i64 + 1, &format!("Shared Name {index}")))
            .collect();
        let mut repository = CompositeRepository::with_definitions(definitions);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("shared"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("too many definitions");

        assert_eq!(error, MutationError::AmbiguousItem);
        // The instance search was never reached, so no partial candidate list
        // could have been truncated into an accidental selection.
        assert_eq!(
            service.repository().recorded_calls(),
            ["load_current", "load_current", "search_definitions"]
        );
    }

    #[test]
    fn plan_move_rejects_a_missing_exact_instance() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("absent"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing instance");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_an_ambiguous_exact_instance() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        repository.seed_instances(vec![
            instance("item-1", 1, ItemLocation::Vault),
            instance("item-1", 1, warlock()),
        ]);
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous instance");

        assert_eq!(error, MutationError::AmbiguousItem);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_builds_one_direct_step_to_the_vault() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), DestinationSelector::vault()),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.source, warlock());
        assert_eq!(plan.destination, ItemLocation::Vault);
        assert_eq!(plan.step_count(), 1);
        let StepAction::Transfer { from, to } = &plan.steps()[0].action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &warlock());
        assert_eq!(to, &ItemLocation::Vault);
        assert!(!plan.effects().via_vault);
    }

    #[test]
    fn plan_move_resolves_a_character_label_case_insensitively() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), character_label("WARLOCK")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.destination, warlock());
    }

    #[test]
    fn plan_move_rejects_an_unknown_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("titan")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("unknown destination");

        assert_eq!(error, MutationError::MissingDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_an_ambiguous_destination_label() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                character("titan-a", Some("Alpha"), Some("Titan")),
                character("titan-b", Some("Bravo"), Some("Titan")),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), character_label("titan")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous destination");

        assert_eq!(error, MutationError::AmbiguousDestination);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_routes_a_character_to_character_move_through_the_vault() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("hunter")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.step_count(), 2);
        let StepAction::Transfer { from, to } = &plan.steps()[0].action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &warlock());
        assert_eq!(to, &ItemLocation::Vault);
        let StepAction::Transfer { from, to } = &plan.steps()[1].action else {
            panic!("expected transfer");
        };
        assert_eq!(from, &ItemLocation::Vault);
        assert_eq!(to, &hunter());
        assert!(plan.effects().via_vault);
    }

    #[test]
    fn plan_move_rejects_a_no_op_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("no-op move");

        assert_eq!(error, MutationError::SameLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn plan_move_rejects_an_unsupported_source_location() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance(
                "item-1",
                1,
                ItemLocation::other("future-location"),
            )],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("unsupported source");

        assert_eq!(error, MutationError::UnsupportedLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn plan_move_rejects_absent_intent_before_any_side_effect() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let mut move_request = request(exact("item-1"), exact_character("warlock"));
        move_request.intent = MoveIntentAuthorization::Absent;

        let error = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect_err("absent intent");

        assert_eq!(error, MutationError::MissingIntent);
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        // Nothing was read, refreshed, or planned: intent is validated first.
        assert!(service.repository().recorded_calls().is_empty());
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_ambiguous_intent_before_any_side_effect() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let mut move_request = request(exact("item-1"), exact_character("warlock"));
        move_request.intent = MoveIntentAuthorization::Ambiguous;

        let error = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect_err("ambiguous intent");

        assert_eq!(error, MutationError::AmbiguousIntent);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert!(service.repository().recorded_calls().is_empty());
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_refreshes_a_stale_projection_and_binds_the_new_revision() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        queue_refresh(
            &shared,
            // One extra instance is a real content change, so the repository
            // advances its revision while item-1 stays in the Vault.
            projection(
                NOW,
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, warlock()),
                ],
            ),
        );
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.revision, SyncRevision::new(2));
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn plan_move_bounds_the_plan_window_by_the_supplied_lifetime() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                lifetime,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.created_at(), NOW);
        assert_eq!(plan.expires_at(), Timestamp::new(NOW.seconds() + 30));
        assert_eq!(
            plan.expires_at().seconds() - plan.created_at().seconds(),
            30
        );
    }

    #[test]
    fn plan_move_reports_clock_overflow_without_spending_a_plan_id() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        // The overflowing timestamp is far in the future, so the ordinary gate
        // still refreshes before the bounded expiry is computed.
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                Timestamp::new(u64::MAX),
            )
            .expect_err("overflowing clock");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ClockOverflow)
        );
        assert_eq!(error.category(), ErrorCategory::ClockOverflow);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_propagates_plan_id_exhaustion() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service: MutationService<SharedGateway, CompositeRepository, ExhaustedPlanIds> =
            MutationService::new(
                ProfileSyncService::new(SharedGateway(Rc::clone(&shared)), repository),
                ExhaustedPlanIds,
            );

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("exhausted ids");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::PlanIdExhausted)
        );
    }

    #[test]
    fn plan_move_propagates_a_refresh_failure_with_its_category() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("provider failure");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ProviderUnavailable)
        );
        assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
    }

    #[test]
    fn plan_move_touches_only_read_seams() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );
        let mut service = service(&shared, repository);

        service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        // The whole planning path is one refresh plus reads: the only write is
        // the repository-owned projection publication, and there is no
        // transfer, execution, or provider-mutation seam to call.
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "replace_current",
                "load_current",
                "search_definitions",
                "search_instances"
            ]
        );
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn plan_move_reports_an_invalid_projection_when_no_snapshot_is_visible() {
        let shared = gateway();
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, CompositeRepository::hiding_current());

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("no visible projection");

        assert_eq!(error, MutationError::InvalidProjection);
        assert_eq!(error.category(), ErrorCategory::LocalStorage);
    }

    #[test]
    fn preflight_confirms_a_valid_plan_and_forces_a_refresh() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        assert_eq!(service.repository().recorded_calls().len(), 3);
        // The preflight refresh re-commits identical content, so the revision is
        // retained and the plan stays valid.
        queue_refresh(&shared, fresh_vault_projection());

        service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect("valid preflight");

        // Preflight refreshed even though the projection was fresh: an ordinary
        // gate would have skipped acquisition here.
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "lookup_instance",
                "load_current",
                "replace_current",
                "load_current",
                "lookup_instance",
                "lookup_instance"
            ]
        );
        assert_eq!(plan.revision, SyncRevision::new(1));
    }

    #[test]
    fn preflight_rejects_absent_intent_before_syncing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut unauthorized = move_request.clone();
        unauthorized.intent = MoveIntentAuthorization::Absent;

        let error = service
            .preflight_move(&unauthorized, &plan, TTL, NOW)
            .expect_err("absent intent");

        assert_eq!(error, MutationError::MissingIntent);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn preflight_rejects_ambiguous_intent_before_syncing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut unauthorized = move_request.clone();
        unauthorized.intent = MoveIntentAuthorization::Ambiguous;

        let error = service
            .preflight_move(&unauthorized, &plan, TTL, NOW)
            .expect_err("ambiguous intent");

        assert_eq!(error, MutationError::AmbiguousIntent);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn preflight_rejects_an_expired_plan() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");
        let plan = service
            .plan_move(&move_request, lifetime, TTL, NOW)
            .expect("plan");
        queue_refresh(&shared, fresh_vault_projection());
        let later = Timestamp::new(NOW.seconds() + 30);

        let error = service
            .preflight_move(&move_request, &plan, TTL, later)
            .expect_err("expired plan");

        assert_eq!(error, MutationError::Expired);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_plan_whose_projection_changed() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        // The item moved to a character, so the committed content changed and
        // the repository advances its revision.
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, warlock())],
            ),
        );

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("changed revision");

        assert_eq!(error, MutationError::StaleRevision);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_vanished_item_resolution() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        // A plan whose item is not in the refreshed projection must be reported
        // as an unresolvable identity instead of being trusted.
        plan.item = item_id("vanished");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("vanished item");

        assert_eq!(error, MutationError::MissingResolution);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_destination_character_that_no_longer_exists() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        plan.destination = hunter();
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("missing destination");

        assert_eq!(error, MutationError::MissingDestination);
    }

    #[test]
    fn preflight_reports_an_unsupported_plan_endpoint() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        plan.destination = ItemLocation::other("future-location");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("unsupported endpoint");

        assert_eq!(error, MutationError::UnsupportedLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn preflight_rejects_a_plan_for_another_membership() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut other = move_request.clone();
        other.membership = destiny_core::MembershipId::new("99999999999999999");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&other, &plan, TTL, NOW)
            .expect_err("wrong membership");

        assert_eq!(error, MutationError::WrongMembership);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_exact_item() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 2, ItemLocation::Vault),
            ],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, ItemLocation::Vault),
                ],
            ),
        );

        // The plan is internally valid, but the request names a different
        // item, so intent for item-2 must not preflight a plan for item-1.
        let error = service
            .preflight_move(
                &request(exact("item-2"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong exact item");

        assert_eq!(error, MutationError::WrongItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_semantic_item() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(vec![
            definition(1, "Gjallarhorn"),
            definition(2, "Thorn"),
        ]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 2, ItemLocation::Vault),
            ],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, ItemLocation::Vault),
                ],
            ),
        );

        // The semantic term resolves uniquely, but to a different concrete
        // instance than the plan binds.
        let error = service
            .preflight_move(
                &request(semantic("thorn"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong semantic item");

        assert_eq!(error, MutationError::WrongItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_exact_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("hunter", Some("Hunter"), None),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        // The request names a different destination than the plan binds.
        let error = service
            .preflight_move(
                &request(exact("item-1"), exact_character("hunter")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong exact destination");

        assert_eq!(error, MutationError::WrongDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_semantic_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("hunter", Some("Hunter"), None),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        // The label resolves uniquely, but to a different character than the
        // plan binds.
        let error = service
            .preflight_move(
                &request(exact("item-1"), character_label("hunter")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong semantic destination");

        assert_eq!(error, MutationError::WrongDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_request_item_absent_from_the_projection() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(
                &request(exact("absent"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("missing requested item");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_an_ambiguous_request_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("titan-a", Some("Alpha"), Some("Titan")),
                character("titan-b", Some("Bravo"), Some("Titan")),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("titan-a", Some("Alpha"), Some("Titan")),
                    character("titan-b", Some("Bravo"), Some("Titan")),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        let error = service
            .preflight_move(
                &request(exact("item-1"), character_label("titan")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("ambiguous requested destination");

        assert_eq!(error, MutationError::AmbiguousDestination);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
    }

    #[test]
    fn preflight_propagates_a_refresh_failure() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        shared
            .borrow_mut()
            .queue_membership(Err(AppError::new(ErrorCategory::ProviderRateLimit)));
        let mut failing = service;
        let error = failing
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("rate limit");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ProviderRateLimit)
        );
    }

    #[test]
    fn policy_outcomes_map_to_specific_typed_errors() {
        let cases = [
            (MutationPolicyOutcome::Valid, MutationError::InvalidPlan),
            (MutationPolicyOutcome::Expired, MutationError::Expired),
            (
                MutationPolicyOutcome::StaleRevision,
                MutationError::StaleRevision,
            ),
            (
                MutationPolicyOutcome::WrongMembership,
                MutationError::WrongMembership,
            ),
            (MutationPolicyOutcome::WrongItem, MutationError::WrongItem),
            (
                MutationPolicyOutcome::WrongDestination,
                MutationError::WrongDestination,
            ),
            (
                MutationPolicyOutcome::MissingResolution,
                MutationError::MissingResolution,
            ),
            (
                MutationPolicyOutcome::AmbiguousResolution,
                MutationError::AmbiguousResolution,
            ),
            (
                MutationPolicyOutcome::MissingDestination,
                MutationError::MissingDestination,
            ),
            (
                MutationPolicyOutcome::AmbiguousDestination,
                MutationError::AmbiguousDestination,
            ),
            (
                MutationPolicyOutcome::UnsupportedLocation,
                MutationError::UnsupportedLocation,
            ),
            (
                MutationPolicyOutcome::UnsupportedSource,
                MutationError::UnsupportedSource,
            ),
            (
                MutationPolicyOutcome::SourceMismatch,
                MutationError::SourceMismatch,
            ),
        ];

        for (outcome, expected) in cases {
            assert_eq!(MutationError::from_policy_outcome(outcome), expected);
        }
    }

    #[test]
    fn prepare_move_plans_and_preflights_in_one_shared_call() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        // Only the mandatory preflight refresh acquires anything: the ordinary
        // planning gate finds the projection fresh and skips acquisition.
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .prepare_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("prepared plan");

        assert_eq!(plan.plan_id, PlanId::new("plan-1"));
        assert_eq!(plan.revision, SyncRevision::new(1));
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(service.plan_ids().calls(), 1);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "lookup_instance",
                "load_current",
                "replace_current",
                "load_current",
                "lookup_instance",
                "lookup_instance"
            ]
        );
    }

    #[test]
    fn prepare_move_rejects_a_plan_the_preflight_invalidated() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, warlock())],
            ),
        );
        let mut service = service(&shared, repository);

        let error = service
            .prepare_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("invalidated plan");

        assert_eq!(error, MutationError::StaleRevision);
        // The invalidated plan is never handed to a caller.
        assert_eq!(service.plan_ids().calls(), 1);
    }

    #[test]
    fn errors_are_sanitized() {
        assert_eq!(
            MutationError::MissingItem.to_string(),
            "the requested item is not in the committed projection"
        );
        assert_eq!(
            MutationError::Application(ErrorCategory::ProviderUnavailable).to_string(),
            "the provider is unavailable"
        );
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("secret-item"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing item");
        let rendered = format!("{error} {error:?}");

        for secret in ["secret-item", MEMBERSHIP_IDENTIFIER, "warlock"] {
            assert!(!rendered.contains(secret), "diagnostic leaked {secret}");
        }
        assert_eq!(AppError::from(error).category(), error.category());
        assert_eq!(
            MutationError::from(AppError::new(ErrorCategory::LocalStorage)),
            { MutationError::Application(ErrorCategory::LocalStorage) }
        );
    }

    #[test]
    fn move_request_carries_no_approval_artifact() {
        let move_request = request(exact("item-1"), exact_character("warlock"));
        // Exhaustive binding without a rest pattern: the request shape is
        // exactly the membership, the two selectors, and the intent fact, so no
        // preview token, confirmation value, or approval artifact can exist.
        let MoveRequest {
            membership,
            item,
            destination,
            intent,
        } = move_request;
        assert_eq!(membership, membership_id());
        assert_eq!(item.instance_id().expect("exact item"), &item_id("item-1"));
        assert_eq!(
            destination,
            DestinationSelector::character(CharacterId::new("warlock"))
        );
        assert_eq!(
            intent.authorize(),
            destiny_core::IntentAuthorizationOutcome::Authorized
        );
    }

    // --- DTE-003: mandatory fresh preflight and ordered execution ---

    /// Plans one direct Vault -> Warlock move against a committed projection.
    fn direct_plan(service: &mut Service) -> (MoveRequest, MovePlan) {
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        (move_request, plan)
    }

    /// Authorizes exactly one execution attempt, expecting a ticket.
    fn authorized_ticket(
        service: &mut Service,
        move_request: &MoveRequest,
        plan: &MovePlan,
    ) -> ExecutionTicket {
        authorized(
            service
                .authorize_execution(move_request, plan, TTL, NOW)
                .expect("authorization"),
        )
        .expect("expected an authorized ticket")
    }

    /// Project the gate result onto the optional ticket without a diverging arm.
    fn authorized(authorization: ExecutionAuthorization) -> Option<ExecutionTicket> {
        match authorization {
            ExecutionAuthorization::Authorized(ticket) => Some(ticket),
            ExecutionAuthorization::VerifiedNoOp(_) => None,
        }
    }

    /// Commits a plan whose item moves from the Vault to a character and returns
    /// the service and its request/plan pair.
    fn direct_service_with_plan() -> (
        Rc<RefCell<QueuedProfileGateway>>,
        Service,
        MoveRequest,
        MovePlan,
    ) {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let (move_request, plan) = direct_plan(&mut service);
        (shared, service, move_request, plan)
    }

    /// Commits a plan whose item moves from one character to another through the
    /// Vault and returns the service and its request/plan pair.
    fn vault_mediated_service_with_plan() -> (
        Rc<RefCell<QueuedProfileGateway>>,
        Service,
        MoveRequest,
        MovePlan,
    ) {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        let projection = projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, warlock())],
        );
        repository.commit(projection.clone());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("hunter"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        queue_refresh(&shared, projection);
        (shared, service, move_request, plan)
    }

    #[test]
    fn authorize_execution_reuses_the_mandatory_fresh_preflight_and_mints_one_ticket() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        // The planning gate found the projection fresh and skipped acquisition;
        // only the mandatory preflight forces one synchronized projection.
        assert_eq!(shared.borrow().inventory_calls(), 0);
        queue_refresh(&shared, fresh_vault_projection());

        let ticket = authorized_ticket(&mut service, &move_request, &plan);

        assert_eq!(ticket.plan_id(), &PlanId::new("plan-1"));
        assert_eq!(ticket.item(), &item_id("item-1"));
        assert_eq!(ticket.source(), &ItemLocation::Vault);
        assert_eq!(ticket.destination(), &warlock());
        assert_eq!(ticket.revision(), SyncRevision::new(1));
        assert_eq!(ticket.step_count(), 1);
        assert_eq!(ticket.steps(), plan.steps());
        // Even a fresh projection was refreshed, because an attempt may only
        // start from a state that was actually re-read.
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn authorize_execution_reports_a_verified_no_op_when_the_item_is_already_there() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        // The fresh preflight proves the concrete item already sits at the
        // plan's destination, so the requested end state already holds.
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, warlock())],
            ),
        );

        let authorization = service
            .authorize_execution(&move_request, &plan, TTL, NOW)
            .expect("authorization");

        match authorization {
            ExecutionAuthorization::VerifiedNoOp(evidence) => {
                assert_eq!(evidence.plan_id(), &PlanId::new("plan-1"));
                assert_eq!(evidence.attempted_count(), 0);
                assert_eq!(evidence.unattempted_count(), 1);
                assert_eq!(evidence.completed_count(), 0);
                assert!(
                    evidence
                        .steps()
                        .iter()
                        .all(|record| record.outcome == StepOutcome::Unattempted)
                );
            }
            other @ ExecutionAuthorization::Authorized(_) => {
                panic!("expected a verified no-op, got {other:?}")
            }
        }
        // No ticket exists on this path, so the executor has no authorization
        // and cannot issue any transfer step: zero provider mutation calls.
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn authorize_execution_rejects_an_expired_plan_without_forcing_a_preflight() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");
        let plan = service
            .plan_move(&move_request, lifetime, TTL, NOW)
            .expect("plan");
        let later = Timestamp::new(NOW.seconds() + 30);

        let error = service
            .authorize_execution(&move_request, &plan, TTL, later)
            .expect_err("expired plan");

        assert_eq!(error, MutationError::Expired);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        // Expiry needs no provider read, so the gate rejects it before the
        // forced refresh: not one membership or inventory call is spent.
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn authorize_execution_rejects_a_plan_whose_projection_changed_away() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![
                    warlock_character(),
                    character("hunter", Some("Hunter"), None),
                ],
                vec![instance("item-1", 1, hunter())],
            ),
        );

        let error = service
            .authorize_execution(&move_request, &plan, TTL, NOW)
            .expect_err("changed projection");

        assert_eq!(error, MutationError::StaleRevision);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn authorize_execution_rejects_a_request_for_another_membership_without_a_preflight() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        let mut other = move_request.clone();
        other.membership = destiny_core::MembershipId::new("99999999999999999");

        let error = service
            .authorize_execution(&other, &plan, TTL, NOW)
            .expect_err("wrong membership");

        assert_eq!(error, MutationError::WrongMembership);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        // The plan already binds a different membership, so the gate rejects the
        // request before the forced refresh touches the provider.
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn authorize_execution_rejects_a_request_for_a_different_item() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        let committed = projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 2, ItemLocation::Vault),
            ],
        );
        repository.commit(committed.clone());
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(&shared, committed);

        let error = service
            .authorize_execution(
                &request(exact("item-2"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong item");

        assert_eq!(error, MutationError::WrongItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn authorize_execution_rejects_absent_intent_before_any_side_effect() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        let mut unauthorized = move_request.clone();
        unauthorized.intent = MoveIntentAuthorization::Absent;

        let error = service
            .authorize_execution(&unauthorized, &plan, TTL, NOW)
            .expect_err("absent intent");

        assert_eq!(error, MutationError::MissingIntent);
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        // Intent is validated before the forced refresh: no provider call, no
        // projection read, and no attempt.
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn authorize_execution_reports_a_vanished_plan_item() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        let mut mutated = plan.clone();
        mutated.item = item_id("vanished");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .authorize_execution(&move_request, &mutated, TTL, NOW)
            .expect_err("vanished item");

        assert_eq!(error, MutationError::MissingResolution);
    }

    #[test]
    fn rc002_move_ticket_rejects_newer_same_account_revision_before_dispatch() {
        for via_vault in [false, true] {
            let (shared, mut svc, request, plan) = if via_vault {
                vault_mediated_service_with_plan()
            } else {
                direct_service_with_plan()
            };
            if !via_vault {
                queue_refresh(&shared, fresh_vault_projection());
            }
            let ticket = authorized_ticket(&mut svc, &request, &plan);
            let mut newer = fresh_vault_projection();
            newer.items[0].locked = true;
            let committed = svc
                .sync_mut()
                .repository_mut()
                .replace_current(newer)
                .unwrap();
            assert_eq!(committed.metadata.membership_id, *ticket.membership());
            assert!(committed.metadata.revision > ticket.revision());
            let mut transfer = QueuedTransferGateway::new();
            transfer.queue(Ok(TransferOutcome::Transferred));
            transfer.queue(Ok(TransferOutcome::Transferred));
            let failure = svc.execute_move(&mut transfer, &plan, ticket).unwrap_err();
            assert_eq!(failure.error(), MutationError::StaleRevision);
            assert_eq!(failure.evidence().attempted_count(), 0);
            assert_eq!(transfer.calls(), 0);
        }
    }

    #[test]
    fn rc002_equip_ticket_rejects_newer_same_account_revision_before_dispatch() {
        use crate::test_support::QueuedEquipGateway;
        use destiny_core::{EquipOutcome, domain::ItemSlot};
        for source in [warlock(), ItemLocation::Vault, hunter()] {
            for drift in [false, true] {
                let shared = gateway();
                let input = projection(
                    NOW,
                    vec![
                        warlock_character(),
                        character("hunter", None, Some("Hunter")),
                    ],
                    vec![instance("candidate", 1, source.clone())],
                );
                let mut repo = CompositeRepository::with_definitions(vec![equip_definition(
                    1,
                    ItemSlot::Kinetic,
                )]);
                repo.commit(input.clone());
                let mut svc = service(&shared, repo);
                let plan = svc
                    .plan_equip(&equip_request(), LIFETIME, TTL, NOW)
                    .unwrap();
                queue_refresh(&shared, input.clone());
                let (_, observation) = svc
                    .fresh_equip_preflight(&equip_request(), &plan, TTL, NOW)
                    .unwrap();
                let ticket =
                    ExecutionTicket::after_fresh_preflight(plan.mutation_plan(), &observation, NOW)
                        .unwrap();
                if drift {
                    let mut newer = input;
                    newer.items[0].locked = true;
                    let committed = svc
                        .sync_mut()
                        .repository_mut()
                        .replace_current(newer)
                        .unwrap();
                    assert_eq!(committed.metadata.membership_id, *ticket.membership());
                    assert!(committed.metadata.revision > ticket.revision());
                }
                let mut transfer = QueuedTransferGateway::new();
                transfer.queue(Ok(TransferOutcome::Transferred));
                transfer.queue(Ok(TransferOutcome::Transferred));
                let mut equip = QueuedEquipGateway::new();
                equip.queue(Ok(EquipOutcome::Equipped));
                let result = svc.execute_equip((&mut transfer, &mut equip), &plan, ticket);
                if drift {
                    let failure = result.unwrap_err();
                    assert_eq!(failure.error(), MutationError::StaleRevision);
                    assert_eq!(failure.evidence().attempted_count(), 0);
                    assert_eq!((transfer.calls(), equip.calls()), (0, 0));
                } else {
                    assert!(result.unwrap().is_complete());
                    assert_eq!(transfer.calls(), plan.steps().len() - 1);
                    assert_eq!(equip.calls(), 1);
                }
            }
        }
    }

    #[test]
    fn execute_move_issues_a_direct_step_in_order_and_records_completion() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert!(evidence.is_complete());
        assert!(!evidence.is_partial());
        assert_eq!(evidence.attempted_count(), 1);
        assert_eq!(evidence.completed_count(), 1);
        assert_eq!(evidence.unattempted_count(), 0);
        assert_eq!(evidence.stop_outcome(), None);
        assert_eq!(evidence.next_ordinal(), None);
        assert_eq!(transfer.calls(), 1);

        let issued = transfer.requests().first().expect("one request");
        assert_eq!(issued.membership().membership_id(), &membership_id());
        assert_eq!(
            issued.membership().membership_type(),
            destiny_core::MembershipType::new(3)
        );
        assert_eq!(issued.item(), &item_id("item-1"));
        assert_eq!(issued.item_hash(), ItemHash::new(1));
        assert_eq!(issued.quantity(), 1);
        assert_eq!(issued.from(), &ItemLocation::Vault);
        assert_eq!(issued.to(), &warlock());
        assert!(!issued.to_vault());
    }

    #[test]
    fn execute_move_routes_a_character_to_character_move_through_the_vault_in_order() {
        let (_shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        transfer.queue(Ok(TransferOutcome::Transferred));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert!(evidence.is_complete());
        assert_eq!(evidence.attempted_count(), 2);
        assert_eq!(evidence.completed_count(), 2);
        assert_eq!(transfer.calls(), 2);
        assert_eq!(plan.step_count(), 2);
        let first = transfer.requests().first().expect("first request");
        let second = transfer.requests().get(1).expect("second request");
        // Exactly the plan's ordered steps: character -> Vault -> character.
        assert_eq!(first.from(), &warlock());
        assert_eq!(first.to(), &ItemLocation::Vault);
        assert_eq!(second.from(), &ItemLocation::Vault);
        assert_eq!(second.to(), &hunter());
    }

    #[test]
    fn execute_move_stops_after_a_first_step_refusal_without_issuing_the_next_step() {
        let (_shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Rejected(
            TransferRejection::NotTransferable,
        )));
        // A second outcome is queued to prove the executor never reaches it.
        transfer.queue(Ok(TransferOutcome::Transferred));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert_eq!(transfer.calls(), 1);
        assert_eq!(evidence.attempted_count(), 1);
        assert_eq!(evidence.completed_count(), 0);
        assert_eq!(evidence.unattempted_count(), 1);
        assert!(!evidence.is_partial());
        assert_eq!(
            evidence.stop_outcome(),
            Some(StepOutcome::Rejected(TransferRejection::NotTransferable))
        );
        assert_eq!(evidence.next_ordinal(), Some(2));
    }

    #[test]
    fn execute_move_reports_partial_completion_when_a_later_step_fails() {
        let (_shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        transfer.queue(Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        )));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert!(evidence.is_partial());
        assert!(!evidence.is_complete());
        assert_eq!(evidence.attempted_count(), 2);
        assert_eq!(evidence.completed_count(), 1);
        assert_eq!(evidence.unattempted_count(), 0);
        assert_eq!(
            evidence.stop_outcome(),
            Some(StepOutcome::Rejected(TransferRejection::DestinationFull))
        );
        assert_eq!(transfer.calls(), 2);
    }

    #[test]
    fn execute_move_records_post_dispatch_uncertainty_as_typed_evidence() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Uncertain));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert_eq!(transfer.calls(), 1);
        assert_eq!(evidence.attempted_count(), 1);
        assert_eq!(evidence.completed_count(), 0);
        assert!(!evidence.is_partial());
        assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Uncertain));
        assert!(evidence.stop_outcome().expect("stop").is_uncertain());
    }

    #[test]
    fn execute_move_records_a_transient_step_outcome() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transient));

        let evidence = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect("execution");

        assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Transient));
        assert!(!evidence.stop_outcome().expect("stop").is_uncertain());
        assert_eq!(transfer.calls(), 1);
    }

    #[test]
    fn execute_move_preserves_a_pre_dispatch_provider_error() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        // An error means no provider mutation could have occurred, so it is
        // preserved rather than recorded as a step outcome.
        transfer.queue(Err(AppError::new(ErrorCategory::ProviderUnavailable)));

        let failure = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect_err("pre-dispatch provider error");

        assert_eq!(
            failure.error(),
            MutationError::Application(ErrorCategory::ProviderUnavailable)
        );
        assert_eq!(
            failure.error().category(),
            ErrorCategory::ProviderUnavailable
        );
        // The first step failed before reaching the provider, so the evidence
        // honestly reports no remote mutation and no post-action refresh.
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert_eq!(failure.evidence().completed_count(), 0);
        assert!(!failure.requires_post_action_refresh());
        assert_eq!(transfer.calls(), 1);
        let rendered = format!("{} {:?}", failure.error(), failure.error());
        for secret in ["item-1", MEMBERSHIP_IDENTIFIER, "warlock"] {
            assert!(!rendered.contains(secret), "diagnostic leaked {secret}");
        }
    }

    #[test]
    fn execute_move_preserves_first_step_evidence_when_a_later_step_fails_before_dispatch() {
        let (_shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        // The second step fails before dispatch, so the first step's applied
        // effect must not be dropped from the failure record.
        transfer.queue(Err(AppError::new(ErrorCategory::ProviderUnavailable)));

        let failure: ExecutionFailure = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect_err("second-step pre-dispatch failure");

        assert_eq!(
            failure.error(),
            MutationError::Application(ErrorCategory::ProviderUnavailable)
        );
        assert_eq!(transfer.calls(), 2);
        assert_eq!(failure.evidence().attempted_count(), 1);
        assert_eq!(failure.evidence().completed_count(), 1);
        assert_eq!(failure.evidence().unattempted_count(), 1);
        assert!(failure.evidence().is_partial());
        assert_eq!(failure.evidence().plan_id(), &PlanId::new("plan-1"));
        // The first step reached the provider, so DTE-004 must refresh before
        // deciding anything about the item's location.
        assert!(failure.requires_post_action_refresh());
    }

    #[test]
    fn execute_move_fails_closed_when_the_ticket_does_not_match_the_plan() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        let mut mutated = plan.clone();
        mutated.plan_id = PlanId::new("other-plan");
        let mut transfer = QueuedTransferGateway::new();

        let failure = service
            .execute_move(&mut transfer, &mutated, ticket)
            .expect_err("mismatched ticket");

        assert_eq!(failure.error(), MutationError::InvalidPlan);
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert!(!failure.requires_post_action_refresh());
        assert_eq!(transfer.calls(), 0);
    }

    #[test]
    fn execute_move_reports_a_missing_item_resolution_without_a_provider_call() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let ticket = authorized_ticket(&mut service, &move_request, &plan);
        // The item vanishes from the search projection after the ticket was
        // minted, so the executor cannot build a request for it.
        service.repository_mut().seed_instances(Vec::new());
        let mut transfer = QueuedTransferGateway::new();

        let failure = service
            .execute_move(&mut transfer, &plan, ticket)
            .expect_err("missing item");

        assert_eq!(failure.error(), MutationError::MissingResolution);
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert_eq!(transfer.calls(), 0);
    }

    #[test]
    fn execute_move_takes_the_ticket_by_value() {
        // The entry point signature takes `ExecutionTicket` by value, not by
        // reference. This function item pins the exact shape at compile time, so
        // a future borrowing or replayable signature fails to compile here. The
        // ticket type is not `Clone`, which is what makes a second attempt from
        // the same authorization impossible rather than merely discouraged.
        type EntryPoint = fn(
            &mut Service,
            &mut QueuedTransferGateway,
            &MovePlan,
            ExecutionTicket,
        ) -> Result<ExecutionEvidence, ExecutionFailure>;
        let _: EntryPoint = Service::execute_move::<QueuedTransferGateway>;
    }

    // --- DTE-004: mandatory post-action refresh and destination verification ---

    fn hunter_character() -> Character {
        character("hunter", Some("Hunter"), None)
    }

    /// One post-action projection that places the concrete item at `location`.
    ///
    /// Both characters are present so a moved item is structurally valid, which
    /// is what lets the verification tests distinguish a destination mismatch
    /// from a structurally invalid projection.
    fn after_projection(location: ItemLocation) -> InventorySnapshotInput {
        projection(
            NOW,
            vec![warlock_character(), hunter_character()],
            vec![instance("item-1", 1, location)],
        )
    }

    #[test]
    fn execute_move_verified_reports_verified_success_for_a_direct_move() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(&shared, after_projection(warlock()));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert!(evidence.is_complete());
                assert_eq!(evidence.attempted_count(), 1);
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 1);
        // Only the mandatory fresh preflight.
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_reports_verified_success_for_a_vault_mediated_move() {
        let (shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        queue_refresh(&shared, after_projection(hunter()));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert!(evidence.is_complete());
                assert_eq!(evidence.attempted_count(), 2);
                assert_eq!(evidence.completed_count(), 2);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 2);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_reports_a_verified_no_op_without_a_post_action_refresh() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        // The fresh preflight proves the concrete item already at the
        // destination, so no step is dispatched.
        queue_refresh(&shared, after_projection(warlock()));
        let mut transfer = QueuedTransferGateway::new();

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::VerifiedNoOp(evidence) => {
                assert_eq!(evidence.attempted_count(), 0);
                assert_eq!(evidence.unattempted_count(), 1);
            }
            other => panic!("expected a verified no-op, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 0);
        // Only the fresh preflight refreshed; no post-action refresh is spent.
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_reports_vault_stranding_as_an_honest_incomplete_execution() {
        let (shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        transfer.queue(Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        )));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(failure) => {
                // The first step applied and the second was refused, so the item
                // is honestly stranded in the Vault; no rollback is claimed.
                assert_eq!(failure.observed(), &ItemLocation::Vault);
                assert!(failure.is_partial());
                assert_eq!(failure.error(), None);
                assert_eq!(
                    failure.stop_outcome(),
                    Some(StepOutcome::Rejected(TransferRejection::DestinationFull))
                );
                assert_eq!(failure.evidence().completed_count(), 1);
                assert_eq!(failure.evidence().attempted_count(), 2);
                assert_eq!(failure.evidence().unattempted_count(), 0);
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 2);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_pending_provider_refresh() {
        for step in [
            TransferOutcome::Transferred,
            TransferOutcome::Uncertain,
            TransferOutcome::AuthenticationRefused,
            TransferOutcome::Rejected(TransferRejection::DestinationFull),
        ] {
            let (shared, mut service, request, plan) = direct_service_with_plan();
            queue_refresh(&shared, fresh_vault_projection());
            let mut stale = fresh_vault_projection();
            stale.checked_at = POST_ACTION_NOW;
            queue_refresh(&shared, stale);
            let mut transfer = QueuedTransferGateway::new();
            transfer.queue(Ok(step));
            let outcome = service
                .execute_move_verified(&mut transfer, &request, &plan, TTL, NOW, POST_ACTION_NOW)
                .unwrap();
            match outcome {
                outcome if step == TransferOutcome::Transferred => {
                    assert!(format!("{outcome:?}").starts_with("Applied("));
                    assert_eq!(shared.borrow().inventory_calls(), 1);
                    service
                        .sync
                        .sync(
                            destiny_core::ProfileSyncReason::Ordinary,
                            TTL,
                            POST_ACTION_NOW,
                        )
                        .unwrap();
                    assert_eq!(shared.borrow().inventory_calls(), 2);
                    assert!(!service.repository().freshness_invalidated().unwrap());
                    assert_eq!(
                        service.committed_snapshot().unwrap().metadata.checked_at,
                        POST_ACTION_NOW
                    );
                }
                MoveExecutionOutcome::IncompleteExecution(incomplete) => {
                    assert_ne!(step, TransferOutcome::Transferred);
                    assert_eq!(incomplete.observed(), &ItemLocation::Vault);
                    assert_eq!(incomplete.evidence().completed_count(), 0);
                }
                other => panic!("unexpected outcome: {other:?}"),
            }
        }
    }

    #[test]
    fn execute_move_verified_fails_verification_when_the_refreshed_state_changed_away() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        // A genuinely changed revision still holds the item at the source.
        let mut changed = after_projection(ItemLocation::Vault);
        changed.characters[0].name = Some("Changed character".into());
        queue_refresh(&shared, changed);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_fails_verification_at_an_unexpected_destination() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        // The item ended up on a different character than requested.
        queue_refresh(&shared, after_projection(hunter()));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_fails_verification_when_the_item_is_missing_after_refresh() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character(), hunter_character()],
                Vec::new(),
            ),
        );
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_fails_verification_when_the_post_action_refresh_fails() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        // The forced post-action refresh fails before any inventory is acquired,
        // so the destination cannot be observed at all.
        shared
            .borrow_mut()
            .queue_membership(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::Applied(evidence) => {
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        // Only the fresh preflight acquired inventory; the post-action refresh
        // could not.
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(shared.borrow().membership_calls(), 1);
    }

    #[test]
    fn execute_move_verified_confirms_an_uncertain_step_the_projection_shows_applied() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(&shared, after_projection(warlock()));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Uncertain));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(incomplete) => {
                assert_eq!(incomplete.stop_outcome(), Some(StepOutcome::Uncertain));
                assert_eq!(incomplete.evidence().completed_count(), 0);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_reports_an_unapplied_uncertain_step_as_an_incomplete_execution() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Uncertain));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(failure) => {
                assert_eq!(failure.observed(), &ItemLocation::Vault);
                assert!(!failure.is_partial());
                assert_eq!(failure.stop_outcome(), Some(StepOutcome::Uncertain));
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_refreshes_after_a_rejected_step() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Rejected(
            TransferRejection::NotTransferable,
        )));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(failure) => {
                assert_eq!(failure.observed(), &ItemLocation::Vault);
                assert!(!failure.is_partial());
                assert_eq!(
                    failure.stop_outcome(),
                    Some(StepOutcome::Rejected(TransferRejection::NotTransferable))
                );
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }
        // A refusal stops without post-action acquisition.
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(transfer.calls(), 1);
    }

    #[test]
    fn execute_move_verified_refreshes_after_a_transient_step() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transient));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(failure) => {
                assert_eq!(failure.observed(), &ItemLocation::Vault);
                assert_eq!(failure.stop_outcome(), Some(StepOutcome::Transient));
                assert_eq!(failure.error(), None);
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_does_not_refresh_after_a_zero_attempt_error() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        let mut transfer = QueuedTransferGateway::new();
        // The only planned step fails before dispatch, so no remote mutation can
        // have occurred.
        transfer.queue(Err(AppError::new(ErrorCategory::ProviderUnavailable)));

        let failure = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect_err("zero-attempt failure");

        assert_eq!(
            failure.error(),
            MutationError::Application(ErrorCategory::ProviderUnavailable)
        );
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert!(!failure.requires_post_action_refresh());
        assert_eq!(transfer.calls(), 1);
        // Only the fresh preflight refreshed; the zero-attempt error must not
        // spend a post-action refresh.
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_refreshes_after_a_mid_sequence_pre_dispatch_error() {
        let (shared, mut service, move_request, plan) = vault_mediated_service_with_plan();
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        // The second step errors before dispatch, so the first step's applied
        // effect must still force the refresh and survive into the outcome.
        transfer.queue(Err(AppError::new(ErrorCategory::ProviderUnavailable)));

        let outcome = service
            .execute_move_verified(
                &mut transfer,
                &move_request,
                &plan,
                TTL,
                NOW,
                POST_ACTION_NOW,
            )
            .expect("verified outcome");

        match outcome {
            MoveExecutionOutcome::IncompleteExecution(failure) => {
                assert_eq!(failure.observed(), &ItemLocation::Vault);
                assert_eq!(
                    failure.error(),
                    Some(MutationError::Application(
                        ErrorCategory::ProviderUnavailable
                    ))
                );
                assert_eq!(failure.evidence().completed_count(), 1);
                assert_eq!(failure.evidence().attempted_count(), 1);
                assert_eq!(failure.evidence().unattempted_count(), 1);
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 2);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn execute_move_verified_returns_an_authorization_failure_without_dispatching() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");
        let plan = service
            .plan_move(&move_request, lifetime, TTL, NOW)
            .expect("plan");
        let later = Timestamp::new(NOW.seconds() + 30);
        let mut transfer = QueuedTransferGateway::new();

        let failure = service
            .execute_move_verified(&mut transfer, &move_request, &plan, TTL, later, later)
            .expect_err("expired plan");

        assert_eq!(failure.error(), MutationError::Expired);
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert!(!failure.requires_post_action_refresh());
        assert_eq!(transfer.calls(), 0);
        // Expiry is rejected before the forced preflight, so nothing refreshed.
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn execute_move_verified_rejects_a_post_action_timestamp_before_the_preflight() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        let mut transfer = QueuedTransferGateway::new();
        // A post-action observation cannot precede the attempt it observes.
        let earlier = Timestamp::new(NOW.seconds() - 1);

        let failure = service
            .execute_move_verified(&mut transfer, &move_request, &plan, TTL, NOW, earlier)
            .expect_err("inverted timestamps");

        assert_eq!(failure.error(), MutationError::InvalidTimestamps);
        assert_eq!(failure.error().category(), ErrorCategory::StaleState);
        assert_eq!(failure.evidence().attempted_count(), 0);
        assert!(!failure.requires_post_action_refresh());
        // The caller's clock input is validated before any side effect.
        assert_eq!(transfer.calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }
}
