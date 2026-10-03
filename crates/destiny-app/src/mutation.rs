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
//! # Verification (DTE-004)
//!
//! [`MutationService::execute_move_verified`] is the shared orchestration that
//! composes the gates above with a mandatory [`ProfileSyncReason::PostAction`]
//! refresh and a concrete destination verification. It forces one refresh, stamped
//! with an explicit post-action observation timestamp distinct from the preflight
//! timestamp, after every attempt whose bounded evidence shows at least one step
//! reached the provider, then resolves the plan's item in the refreshed committed
//! projection. A uniquely observed item at the destination is a verified success;
//! a location consistent with the sequence's own reported progress is an honest
//! [`IncompleteExecution`] that never claims a rollback; a contradiction, an
//! unresolvable item, or a failed refresh is a [`VerificationFailure`]. A
//! first-step pre-dispatch failure with zero attempted steps is returned as an
//! error without spending a refresh, and the HTTP adapter remains DTE-005.

use std::fmt;

use crate::sync::ProfileSyncService;
use destiny_core::{
    AppError, DefinitionSearchQuery, DestinationSelector, DestinyMembership, ErrorCategory,
    ExecutionEvidence, ExecutionTicket, FreshnessPolicy, InstanceSearchQuery,
    IntentAuthorizationOutcome, InventoryRepository, InventorySnapshot, ItemHashFilter,
    ItemInstance, ItemInstanceId, ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT,
    MovePlan, MovePlanValidation, MoveRequest, MutationPolicyInput, MutationPolicyOutcome,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncOutcome, ProfileSyncReason,
    Resolution, SearchRepository, StepOutcome, SyncRevision, Timestamp, TransferGateway,
    TransferRequest, evaluate_mutation, resolve_destination,
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
            Self::SameLocation | Self::UnsupportedLocation | Self::UnsupportedSource => {
                ErrorCategory::UnsupportedOperation
            }
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
/// and left the concrete item somewhere other than the requested destination.
///
/// The record preserves the bounded [`ExecutionEvidence`] observed before the
/// attempt stopped, the concrete location the refreshed committed projection
/// places the item at, and the pre-dispatch provider error when the sequence
/// stopped on one rather than on a classified step outcome. The concept is
/// deliberately *incomplete* rather than *partial*: zero, one, or every planned
/// step may have applied before the sequence stopped, and
/// [`IncompleteExecution::is_partial`] is what reports whether any step applied.
/// It never claims a rollback: an applied step stays applied, so an item may
/// honestly be stranded at an intermediate location such as the Vault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncompleteExecution {
    error: Option<MutationError>,
    observed: ItemLocation,
    evidence: Box<ExecutionEvidence>,
}

impl IncompleteExecution {
    fn new(
        error: Option<MutationError>,
        observed: ItemLocation,
        evidence: ExecutionEvidence,
    ) -> Self {
        Self {
            error,
            observed,
            evidence: Box::new(evidence),
        }
    }

    /// The pre-dispatch provider error that stopped the sequence, when the
    /// sequence stopped on an error instead of on a classified step outcome.
    pub fn error(&self) -> Option<MutationError> {
        self.error
    }

    /// The concrete location the refreshed projection places the item at.
    pub fn observed(&self) -> &ItemLocation {
        &self.observed
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
    /// A false value means the refreshed projection confirms that no step
    /// applied, but the execution is still *incomplete* rather than successful,
    /// so this predicate must not be used to tell failure from success. It only
    /// distinguishes an incomplete execution that left a provider effect behind
    /// from one that applied nothing; the observed location is reported and no
    /// rollback is claimed either way.
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
    /// The item resolved to a concrete location that contradicts the progress
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

/// The typed, sanitized final outcome of one verified move attempt.
///
/// [`MutationService::execute_move_verified`] combines the DTE-003 attempt with
/// the mandatory [`ProfileSyncReason::PostAction`] refresh and the concrete
/// destination verification, so a caller never has to remember either gate. The
/// four variants are the only terminal shapes that combination can report, and
/// every one carries the bounded [`ExecutionEvidence`] that was observed:
///
/// - [`MoveExecutionOutcome::VerifiedSuccess`]: the refreshed projection proves
///   the concrete item at the requested destination. A step whose effect was
///   reported as uncertain but which the projection confirms also lands here.
/// - [`MoveExecutionOutcome::VerifiedNoOp`]: the mandatory fresh preflight proved
///   the item already at the destination, so zero steps were dispatched and no
///   post-action refresh was needed.
/// - [`MoveExecutionOutcome::IncompleteExecution`]: a dispatched sequence
///   stopped short and the refreshed projection is consistent with the progress
///   it reported, so the item is honestly somewhere other than the destination
///   and may be stranded. Zero or more steps may have applied; the carried record
///   reports which through [`IncompleteExecution::is_partial`]. No rollback is
///   claimed.
/// - [`MoveExecutionOutcome::VerificationFailed`]: a dispatched sequence occurred
///   but the refreshed projection cannot prove the destination.
///
/// A first-step pre-dispatch failure whose evidence has zero attempted steps is
/// not one of these variants: it is returned as `Err(ExecutionFailure)` because
/// no transfer step reached the provider, so there is nothing to refresh or
/// verify. Every failure with at least one attempted step is refreshed and
/// classified into this enum instead.
#[derive(Debug, Eq, PartialEq)]
pub enum MoveExecutionOutcome {
    /// The refreshed committed projection proves the concrete item at the
    /// requested destination.
    VerifiedSuccess(ExecutionEvidence),
    /// The mandatory fresh preflight proved the concrete item already at the
    /// destination, so no transfer step was dispatched.
    VerifiedNoOp(ExecutionEvidence),
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
        match self.run_steps(transfer, plan, &ticket, &mut evidence) {
            Ok(()) => Ok(evidence),
            Err(error) => Err(ExecutionFailure::new(error, evidence)),
        }
    }

    /// DTE-004: the shared orchestration that dispatches one plan and then
    /// verifies the concrete destination from a mandatory fresh post-action
    /// refresh.
    ///
    /// It composes the three existing pieces rather than reimplementing any of
    /// them: [`MutationService::authorize_execution`] performs the mandatory
    /// fresh execution preflight, [`MutationService::execute_move`] issues the
    /// ticket's ordered steps through the unchanged narrow executor seam, and
    /// then one [`ProfileSyncReason::PostAction`] refresh is forced before the
    /// final classification.
    ///
    /// The refresh is mandatory for every dispatched attempt, whether the
    /// attempt reported a rejection, a transient or uncertain result, a partial
    /// completion, or stopped on a mid-sequence pre-dispatch error after an
    /// earlier step applied; that is why the predicate used to decide is
    /// [`ExecutionFailure::requires_post_action_refresh`]. A first-step
    /// pre-dispatch error has zero attempted steps, so no transfer reached the
    /// provider, no remote mutation occurred, and no post-action refresh is
    /// spent: it is returned unchanged as `Err`.
    ///
    /// Verification resolves the plan's concrete item in the freshly refreshed
    /// committed projection. A uniquely observed item at the plan destination is
    /// [`MoveExecutionOutcome::VerifiedSuccess`]. When it is not at the
    /// destination, the observed location is compared with the location the
    /// sequence's own reported progress would leave the item at: a match is an
    /// honest [`MoveExecutionOutcome::IncompleteExecution`] that carries the
    /// bounded evidence and the observed location, while a contradiction, a
    /// missing or ambiguous item, or a failed refresh is
    /// [`MoveExecutionOutcome::VerificationFailed`]. No path claims a rollback,
    /// and the bounded [`ExecutionEvidence`] is preserved in every variant.
    ///
    /// The attempt and its post-action observation use two explicit timestamps.
    /// `preflight_now` authorizes and time-bounds the attempt, while
    /// `post_action_now` is the observation instant of the mandatory
    /// [`ProfileSyncReason::PostAction`] refresh; a post-action instant that
    /// precedes the preflight instant is a caller contract violation and is
    /// rejected with [`MutationError::InvalidTimestamps`] before any provider
    /// read, so the refresh can never be stamped with a time that predates the
    /// action it is meant to observe.
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

        Ok(self.verify_attempt(plan, evidence, attempt_error, policy, post_action_now))
    }

    /// Forces the mandatory post-action refresh and classifies the destination.
    ///
    /// The refresh outcome is never optional, and its failure is itself a
    /// verification failure rather than a propagated error, because a dispatched
    /// sequence must always keep its bounded evidence. The refreshed projection
    /// then decides between verified success, an honest incomplete execution that
    /// is consistent with the reported progress, and a verification failure.
    fn verify_attempt(
        &mut self,
        plan: &MovePlan,
        evidence: ExecutionEvidence,
        attempt_error: Option<MutationError>,
        policy: FreshnessPolicy,
        post_action_now: Timestamp,
    ) -> MoveExecutionOutcome {
        // The post-action refresh is unconditional: no classification may read
        // a projection that predates the attempt.
        if let Err(app_error) =
            self.sync
                .sync(ProfileSyncReason::PostAction, policy, post_action_now)
        {
            return MoveExecutionOutcome::VerificationFailed(VerificationFailure::new(
                VerificationFailureReason::RefreshFailed,
                None,
                Some(MutationError::from(app_error)),
                evidence,
            ));
        }
        let observed = match self.repository().lookup_instance(&plan.item) {
            Ok(Resolution::Unique(instance)) => instance.location,
            Ok(Resolution::Missing) | Ok(Resolution::Ambiguous(_)) => {
                return MoveExecutionOutcome::VerificationFailed(VerificationFailure::new(
                    VerificationFailureReason::Unresolved,
                    None,
                    attempt_error,
                    evidence,
                ));
            }
            Err(app_error) => {
                return MoveExecutionOutcome::VerificationFailed(VerificationFailure::new(
                    VerificationFailureReason::RefreshFailed,
                    None,
                    Some(MutationError::from(app_error)),
                    evidence,
                ));
            }
        };
        if observed == plan.destination {
            // The requested end state holds, whatever the steps individually
            // reported: a step whose effect was uncertain is now confirmed.
            return MoveExecutionOutcome::VerifiedSuccess(evidence);
        }
        // The item is not at the destination. It is an honest incomplete
        // execution only when the refreshed location matches the progress the
        // sequence reported; any other location contradicts it and is a
        // verification failure. Neither shape rolls an applied step back.
        if observed == expected_location(plan, evidence.completed_count()) {
            return MoveExecutionOutcome::IncompleteExecution(IncompleteExecution::new(
                attempt_error,
                observed,
                evidence,
            ));
        }
        MoveExecutionOutcome::VerificationFailed(VerificationFailure::new(
            VerificationFailureReason::Elsewhere,
            Some(observed),
            attempt_error,
            evidence,
        ))
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
        plan: &MovePlan,
        ticket: &ExecutionTicket,
        evidence: &mut ExecutionEvidence,
    ) -> Result<(), MutationError> {
        // The ticket is the authorization and the plan supplies the bounded
        // evidence skeleton, so the two must describe the same authorized move.
        // A mismatch can only come from a contract violation, and it fails closed
        // before any provider call.
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
            let request = TransferRequest::new(
                membership.clone(),
                ticket.item().clone(),
                instance.item_hash,
                instance.quantity,
                step.from.clone(),
                step.to.clone(),
            )
            .map_err(MutationError::from)?;
            let outcome = transfer.transfer(&request).map_err(MutationError::from)?;
            evidence
                .record(step.ordinal, StepOutcome::from_transfer(outcome))
                .map_err(MutationError::from)?;
            if !outcome.applied() {
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
            ItemLocation::Other(_) => Resolution::Missing,
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
    fn committed_snapshot(&self) -> Result<InventorySnapshot, MutationError> {
        self.repository()
            .load_current()
            .map_err(MutationError::from)?
            .ok_or(MutationError::InvalidProjection)
    }

    /// Resolves the requested item to exactly one concrete instance.
    fn resolve_item(&self, request: &MoveRequest) -> Result<ItemInstance, MutationError> {
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

/// Rejects absent or ambiguous explicit intent before any side effect.
fn authorize(request: &MoveRequest) -> Result<(), MutationError> {
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
        .map(|step| step.to.clone())
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
        SourceMarker, StepOutcome, SyncRevision, Timestamp, TransferOutcome, TransferRejection,
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
        inventory: InMemoryInventoryRepository,
        search: InMemorySearchRepository,
        hides_current: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions),
                hides_current: false,
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
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.record("load_current");
            if self.hides_current {
                return Ok(None);
            }
            self.inventory.load_current()
        }

        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.record("replace_current");
            let snapshot = self.inventory.replace_current(input)?;
            self.search.set_instances(snapshot.items.clone());
            Ok(snapshot)
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

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
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
        }
    }

    fn warlock_character() -> Character {
        character("warlock", Some("Warlock"), None)
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
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
        assert_eq!(plan.steps()[0].from, ItemLocation::Vault);
        assert_eq!(plan.steps()[0].to, warlock());
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
        assert_eq!(plan.steps()[0].from, warlock());
        assert_eq!(plan.steps()[0].to, ItemLocation::Vault);
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
        assert_eq!(plan.steps()[0].from, warlock());
        assert_eq!(plan.steps()[0].to, ItemLocation::Vault);
        assert_eq!(plan.steps()[1].from, ItemLocation::Vault);
        assert_eq!(plan.steps()[1].to, hunter());
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
            MoveExecutionOutcome::VerifiedSuccess(evidence) => {
                assert!(evidence.is_complete());
                assert_eq!(evidence.attempted_count(), 1);
                assert_eq!(evidence.completed_count(), 1);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 1);
        // The mandatory fresh preflight plus the mandatory post-action refresh.
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
            MoveExecutionOutcome::VerifiedSuccess(evidence) => {
                assert!(evidence.is_complete());
                assert_eq!(evidence.attempted_count(), 2);
                assert_eq!(evidence.completed_count(), 2);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(transfer.calls(), 2);
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
        assert_eq!(shared.borrow().inventory_calls(), 2);
    }

    #[test]
    fn execute_move_verified_fails_verification_when_the_refreshed_state_changed_away() {
        let (shared, mut service, move_request, plan) = direct_service_with_plan();
        queue_refresh(&shared, fresh_vault_projection());
        // The step reported success, yet the refreshed projection still holds the
        // item at the source, so the claimed completion is contradicted.
        queue_refresh(&shared, after_projection(ItemLocation::Vault));
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
            MoveExecutionOutcome::VerificationFailed(failure) => {
                assert_eq!(failure.reason(), VerificationFailureReason::Elsewhere);
                assert_eq!(failure.observed(), Some(&ItemLocation::Vault));
                assert_eq!(failure.error(), None);
                assert_eq!(failure.evidence().completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
            MoveExecutionOutcome::VerificationFailed(failure) => {
                assert_eq!(failure.reason(), VerificationFailureReason::Elsewhere);
                assert_eq!(failure.observed(), Some(&hunter()));
                assert_eq!(failure.evidence().completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
            MoveExecutionOutcome::VerificationFailed(failure) => {
                assert_eq!(failure.reason(), VerificationFailureReason::Unresolved);
                assert_eq!(failure.observed(), None);
                assert_eq!(failure.evidence().completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
            MoveExecutionOutcome::VerificationFailed(failure) => {
                assert_eq!(failure.reason(), VerificationFailureReason::RefreshFailed);
                assert_eq!(failure.observed(), None);
                assert_eq!(
                    failure.error(),
                    Some(MutationError::Application(
                        ErrorCategory::ProviderUnavailable
                    ))
                );
                // The dispatched evidence survives the failed verification.
                assert_eq!(failure.evidence().completed_count(), 1);
            }
            other => panic!("expected verification failure, got {other:?}"),
        }
        // Only the fresh preflight acquired inventory; the post-action refresh
        // could not.
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(shared.borrow().membership_calls(), 2);
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
            MoveExecutionOutcome::VerifiedSuccess(evidence) => {
                // The dispatch account keeps the uncertain step honest, while the
                // verified outcome records that the end state actually holds.
                assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Uncertain));
                assert_eq!(evidence.completed_count(), 0);
            }
            other => panic!("expected verified success, got {other:?}"),
        }
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
        // A rejection still reached the provider, so the refresh is mandatory.
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
        assert_eq!(shared.borrow().inventory_calls(), 2);
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
