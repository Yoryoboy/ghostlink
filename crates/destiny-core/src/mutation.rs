//! Pure, provider-neutral mutation planning and safety policy.

use crate::{
    domain::ItemLocation,
    ids::{CharacterId, ItemInstanceId, MembershipId, PlanId, SyncRevision},
    state::{Resolution, Timestamp},
};

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
}

pub type MoveIntent = MovePlan;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovePlanValidation {
    InvalidTimestamps,
    SameLocation,
}

impl MovePlan {
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
        if source == destination {
            return Err(MovePlanValidation::SameLocation);
        }
        Ok(Self {
            plan_id,
            membership,
            item,
            source,
            destination,
            revision,
            created_at,
            expires_at,
        })
    }

    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }

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
        if !supported_location(&self.source) {
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
        if input.resolved_destination.as_ref() != Some(&self.destination) {
            return PlanBinding::WrongDestination;
        }
        if !supported_location(&self.source) || !supported_location(&self.destination) {
            return PlanBinding::UnsupportedLocation;
        }
        PlanBinding::Valid
    }
}

fn supported_location(location: &ItemLocation) -> bool {
    matches!(location, ItemLocation::Vault | ItemLocation::Character(_))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanBinding {
    Valid,
    Expired,
    StaleRevision,
    WrongMembership,
    WrongItem,
    WrongDestination,
    UnsupportedLocation,
    UnsupportedSource,
    SourceMismatch,
    MissingResolution,
    AmbiguousResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationPolicyInput {
    pub now: Timestamp,
    pub membership: MembershipId,
    pub current_revision: SyncRevision,
    pub current_source: ItemLocation,
    pub resolved_item: Resolution<ItemInstanceId>,
    pub resolved_destination: Option<ItemLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationPolicyOutcome {
    Valid,
    Expired,
    StaleRevision,
    WrongMembership,
    WrongItem,
    WrongDestination,
    UnsupportedLocation,
    UnsupportedSource,
    SourceMismatch,
    AmbiguousResolution,
    MissingResolution,
}

pub fn evaluate_mutation(plan: &MovePlan, input: &MutationPolicyInput) -> MutationPolicyOutcome {
    match plan.binding(input) {
        PlanBinding::Valid => MutationPolicyOutcome::Valid,
        PlanBinding::Expired => MutationPolicyOutcome::Expired,
        PlanBinding::StaleRevision => MutationPolicyOutcome::StaleRevision,
        PlanBinding::WrongMembership => MutationPolicyOutcome::WrongMembership,
        PlanBinding::WrongItem => MutationPolicyOutcome::WrongItem,
        PlanBinding::WrongDestination => MutationPolicyOutcome::WrongDestination,
        PlanBinding::UnsupportedLocation => MutationPolicyOutcome::UnsupportedLocation,
        PlanBinding::UnsupportedSource => MutationPolicyOutcome::UnsupportedSource,
        PlanBinding::SourceMismatch => MutationPolicyOutcome::SourceMismatch,
        PlanBinding::AmbiguousResolution => MutationPolicyOutcome::AmbiguousResolution,
        PlanBinding::MissingResolution => MutationPolicyOutcome::MissingResolution,
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
        ids::{CharacterId, PlanId},
    };

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
            resolved_destination: Some(ItemLocation::Character(CharacterId::new("character"))),
        }
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
    fn binding_rejects_revision_and_semantic_mismatches() {
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
                    resolved_destination: Some(ItemLocation::Vault),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongDestination
        );
    }

    #[test]
    fn validates_plan_time_window_and_source_binding() {
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

        for (source, destination) in [
            (ItemLocation::Vault, ItemLocation::Vault),
            (
                ItemLocation::Character(CharacterId::new("same")),
                ItemLocation::Character(CharacterId::new("same")),
            ),
        ] {
            let no_op = MovePlan::new(
                PlanId::new("plan"),
                MembershipId::new("membership"),
                ItemInstanceId::new("item"),
                source,
                destination,
                SyncRevision::new(4),
                Timestamp::new(10),
                Timestamp::new(20),
            );
            assert_eq!(no_op, Err(MovePlanValidation::SameLocation));
        }

        for (source, destination) in [
            (
                ItemLocation::Vault,
                ItemLocation::Character(CharacterId::new("character")),
            ),
            (
                ItemLocation::Character(CharacterId::new("source")),
                ItemLocation::Character(CharacterId::new("destination")),
            ),
        ] {
            assert!(
                MovePlan::new(
                    PlanId::new("plan"),
                    MembershipId::new("membership"),
                    ItemInstanceId::new("item"),
                    source,
                    destination,
                    SyncRevision::new(4),
                    Timestamp::new(10),
                    Timestamp::new(20),
                )
                .is_ok()
            );
        }

        let plan = plan();
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
    }

    #[test]
    fn policy_reports_resolution_and_location_outcomes() {
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
                        ItemInstanceId::new("b")
                    ]),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::AmbiguousResolution
        );
        let unsupported = MovePlan::new(
            PlanId::new("plan"),
            MembershipId::new("membership"),
            ItemInstanceId::new("item"),
            ItemLocation::Other("unknown".into()),
            ItemLocation::Character(CharacterId::new("character")),
            SyncRevision::new(4),
            Timestamp::new(10),
            Timestamp::new(20),
        )
        .unwrap();
        assert_eq!(
            evaluate_mutation(&unsupported, &input(10)),
            MutationPolicyOutcome::UnsupportedLocation
        );
    }
}
