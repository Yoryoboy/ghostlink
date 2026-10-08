//! Pure non-instanced Postmaster pull contracts; no I/O or action authorization.
//!
//! The wire sentinel cannot select a particular stack. Eligibility requires all
//! same-hash observations for the owner, never merged quantities. Callers must
//! establish complete, fresh coverage and qualified destination metadata before
//! evaluating; neither request construction nor this policy proves those facts.

use crate::{
    AppError, CharacterId, DestinyMembership, ErrorCategory, ItemHash, StackContainer,
    StackObservation,
};

/// Immutable wire-representable intent, not proof of ownership or eligibility.
/// No genuine instance ID is fabricated for an anonymous stack.
///
/// ```compile_fail
/// use destiny_core::StackPullRequest;
/// fn alter(request: &mut StackPullRequest) { request.quantity = 0; }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackPullRequest {
    membership: DestinyMembership,
    item_hash: ItemHash,
    character: CharacterId,
    quantity: i32,
}

impl StackPullRequest {
    /// Legacy non-instanced wire sentinel, never a genuine instance identifier.
    pub const ITEM_ID: &'static str = "0";

    /// Validates only wire representability: canonical positive int64 character,
    /// uint32 hash (including zero), int32 membership type, positive int32 amount.
    /// Membership ID retains the existing membership constructor contract and is
    /// not a pull body field. Errors are value-free, sanitized validation errors.
    pub fn new(
        membership: DestinyMembership,
        item_hash: ItemHash,
        character: CharacterId,
        amount: i64,
    ) -> Result<Self, AppError> {
        let id = character.as_str();
        if id.is_empty()
            || id.len() > 19
            || id.starts_with('0')
            || !id.bytes().all(|byte| byte.is_ascii_digit())
            || id.parse::<i64>().is_err()
            || u32::try_from(item_hash.value()).is_err()
            || i32::try_from(membership.membership_type().value()).is_err()
            || !(1..=i64::from(i32::MAX)).contains(&amount)
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            item_hash,
            character,
            quantity: amount as i32,
        })
    }

    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn item_hash(&self) -> ItemHash {
        self.item_hash
    }
    pub const fn character(&self) -> &CharacterId {
        &self.character
    }
    /// Positive signed-int32 wire stackSize, unchanged from the validated amount.
    pub const fn quantity(&self) -> i32 {
        self.quantity
    }
    /// Fixed wire itemId; adapters must not invent an instance identifier.
    pub const fn item_id(&self) -> &'static str {
        Self::ITEM_ID
    }
}

/// Minimal deterministic provider refusal; no transfer-specific code assumptions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullRejection {
    /// Received rejection with proven non-application.
    InvalidRequest,
}
impl StackPullRejection {
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::InvalidRequest => ErrorCategory::ProviderValidation,
        }
    }
}

/// Evidence of one attempt, not recovery. No variant permits automatic retry.
/// Verification belongs to bounded fresh observations: destination same-hash
/// total must rise by exactly the amount and Postmaster total fall by exactly it.
/// No provider-code mapping or verification machinery lives in this contract.
///
/// ```compile_fail
/// use destiny_core::StackPullOutcome;
/// StackPullOutcome::Acknowledged.verified();
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullOutcome {
    /// Provider acknowledgement only, never verified application or recovery.
    Acknowledged,
    Rejected(StackPullRejection),
    /// Received authentication refusal after dispatch; category remains
    /// ProviderAuthentication. Pre-dispatch authentication failure is Err.
    AuthenticationRefused,
    /// Received deterministic transient refusal with proven non-application.
    /// Unknown HTTP 5xx is not this variant without independent evidence.
    Transient,
    /// Dispatch occurred, effect unknown (including lost responses/unknown 5xx).
    Uncertain,
}

/// Explicit destination evidence supplied by the caller, not acquired here.
/// All missing fields refuse; zero observed holdings are `Some(0)`, not `None`.
/// Counts use u64 so aggregation is not narrowed to wire quantity width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackPullCapacityInput {
    /// Qualified destination bucket slot capacity; None includes unknown bucket.
    pub capacity: Option<u64>,
    /// Number of occupied slots in that bucket, NOT its total item quantity.
    pub occupied: Option<u64>,
    /// Complete same-hash quantity in the destination, without source overlap.
    pub existing_quantity: Option<u64>,
    /// Exact manifest int32 metadata; nonpositive values are invalid.
    pub max_stack_size: Option<i32>,
    /// True iff the known stackUniqueLabel is non-empty (without trimming).
    pub unique_stack: Option<bool>,
    /// Account-wide buckets/profile currencies are initially unsupported.
    pub account_wide: Option<bool>,
    /// Exact doesPostmasterPullHaveSideEffects; only explicit false is safe.
    pub side_effects: Option<bool>,
}

/// A local policy decision, NOT dispatch authority or freshness certification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullDecision {
    Eligible,
    Refused(StackPullRefusal),
}

/// Bounded refusal reasons; absence of evidence is not evidence of fullness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullRefusal {
    NoPostmasterStack,
    ExceedsSmallestPostmasterStack,
    UnknownCapacity,
    UnknownOccupancy,
    UnknownExistingQuantity,
    UnknownMaxStackSize,
    UnknownUniqueness,
    UnknownAccountWide,
    UnknownSideEffects,
    AccountWideDestination,
    PullHasSideEffects,
    /// Nonpositive maxStackSize or unique holdings greater than one maximum.
    InvalidCapacityMetadata,
    ArithmeticOverflow,
    InsufficientCapacity,
}

/// Evaluates selection first, then guaranteed-only capacity. The caller supplies
/// all Postmaster stacks for this request's membership/owner; this function cannot
/// detect incomplete coverage. Other owners, hashes and containers do not count.
/// Duplicate rows remain significant; no summing, deduplication or merging.
pub fn evaluate_stack_pull(
    request: &StackPullRequest,
    observations: &[StackObservation],
    capacity: &StackPullCapacityInput,
) -> StackPullDecision {
    let smallest = observations
        .iter()
        .filter(|row| {
            row.item_hash() == request.item_hash()
                && matches!(row.container(), StackContainer::Postmaster(owner) if owner == request.character())
        })
        .map(StackObservation::quantity)
        .min();
    match smallest {
        None => StackPullDecision::Refused(StackPullRefusal::NoPostmasterStack),
        Some(minimum) if request.quantity() > minimum => {
            StackPullDecision::Refused(StackPullRefusal::ExceedsSmallestPostmasterStack)
        }
        Some(_) => evaluate_stack_pull_capacity(request, capacity),
    }
}

/// Guaranteed capacity only: no optimistic rearrangement or "could make space".
/// Open slots saturate at zero if occupancy exceeds capacity. Unique stacks only
/// top off one stack (or require an open slot when absent). Non-unique holdings
/// contribute only their partial-stack room; exact multiples contribute none.
/// Checked multiplication AND addition refuse overflow rather than wrapping.
/// This is only capacity policy, not selection or authorization.
pub fn evaluate_stack_pull_capacity(
    request: &StackPullRequest,
    input: &StackPullCapacityInput,
) -> StackPullDecision {
    match guaranteed_room(input) {
        Ok(room) if request.quantity() as u64 <= room => StackPullDecision::Eligible,
        Ok(_) => StackPullDecision::Refused(StackPullRefusal::InsufficientCapacity),
        Err(reason) => StackPullDecision::Refused(reason),
    }
}

fn guaranteed_room(input: &StackPullCapacityInput) -> Result<u64, StackPullRefusal> {
    use StackPullRefusal::*;
    if input.side_effects.ok_or(UnknownSideEffects)? {
        return Err(PullHasSideEffects);
    }
    if input.account_wide.ok_or(UnknownAccountWide)? {
        return Err(AccountWideDestination);
    }
    let capacity = input.capacity.ok_or(UnknownCapacity)?;
    let occupied = input.occupied.ok_or(UnknownOccupancy)?;
    let existing = input.existing_quantity.ok_or(UnknownExistingQuantity)?;
    let max = input.max_stack_size.ok_or(UnknownMaxStackSize)?;
    if max <= 0 {
        return Err(InvalidCapacityMetadata);
    }
    let max = max as u64;
    let unique = input.unique_stack.ok_or(UnknownUniqueness)?;
    let open = capacity.saturating_sub(occupied);
    if unique {
        return if existing == 0 {
            Ok(if open >= 1 { max } else { 0 })
        } else {
            max.checked_sub(existing).ok_or(InvalidCapacityMetadata)
        };
    }
    if max == 1 {
        return Ok(open);
    }
    let remainder = existing % max;
    let partial_room = if remainder == 0 { 0 } else { max - remainder };
    open.checked_mul(max)
        .and_then(|room| room.checked_add(partial_room))
        .ok_or(ArithmeticOverflow)
}
