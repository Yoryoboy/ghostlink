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
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

pub type MoveIntent = MovePlan;

impl MovePlan {
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now > self.expires_at
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
    MissingResolution,
    AmbiguousResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationPolicyInput {
    pub now: Timestamp,
    pub membership: MembershipId,
    pub current_revision: SyncRevision,
    pub resolved_item: Resolution<ItemInstanceId>,
    pub resolved_destination: Option<ItemLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationPolicyOutcome {
    Valid,
    Expired,
    StaleRevision,
    WrongItem,
    WrongDestination,
    UnsupportedLocation,
    AmbiguousResolution,
    MissingResolution,
}

pub fn evaluate_mutation(
    plan: Option<&MovePlan>,
    input: &MutationPolicyInput,
) -> MutationPolicyOutcome {
    let Some(plan) = plan else {
        return MutationPolicyOutcome::MissingResolution;
    };
    match plan.binding(input) {
        PlanBinding::Valid => MutationPolicyOutcome::Valid,
        PlanBinding::Expired => MutationPolicyOutcome::Expired,
        PlanBinding::StaleRevision => MutationPolicyOutcome::StaleRevision,
        PlanBinding::WrongItem => MutationPolicyOutcome::WrongItem,
        PlanBinding::WrongDestination | PlanBinding::WrongMembership => {
            MutationPolicyOutcome::WrongDestination
        }
        PlanBinding::UnsupportedLocation => MutationPolicyOutcome::UnsupportedLocation,
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
        MovePlan {
            plan_id: PlanId::new("plan"),
            membership: MembershipId::new("membership"),
            item: ItemInstanceId::new("item"),
            source: ItemLocation::Vault,
            destination: ItemLocation::Character(CharacterId::new("character")),
            revision: SyncRevision::new(4),
            created_at: Timestamp::new(10),
            expires_at: Timestamp::new(20),
        }
    }

    fn input(now: u64) -> MutationPolicyInput {
        MutationPolicyInput {
            now: Timestamp::new(now),
            membership: MembershipId::new("membership"),
            current_revision: SyncRevision::new(4),
            resolved_item: Resolution::Unique(ItemInstanceId::new("item")),
            resolved_destination: Some(ItemLocation::Character(CharacterId::new("character"))),
        }
    }

    #[test]
    fn expiry_boundary_is_valid_then_expired() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(Some(&plan), &input(20)),
            MutationPolicyOutcome::Valid
        );
        assert_eq!(
            evaluate_mutation(Some(&plan), &input(21)),
            MutationPolicyOutcome::Expired
        );
    }

    #[test]
    fn binding_rejects_revision_and_semantic_mismatches() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(
                Some(&plan),
                &MutationPolicyInput {
                    current_revision: SyncRevision::new(5),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::StaleRevision
        );
        assert_eq!(
            evaluate_mutation(
                Some(&plan),
                &MutationPolicyInput {
                    resolved_item: Resolution::Unique(ItemInstanceId::new("other")),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongItem
        );
        assert_eq!(
            evaluate_mutation(
                Some(&plan),
                &MutationPolicyInput {
                    resolved_destination: Some(ItemLocation::Vault),
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::WrongDestination
        );
    }

    #[test]
    fn policy_reports_resolution_and_location_outcomes() {
        let plan = plan();
        assert_eq!(
            evaluate_mutation(
                Some(&plan),
                &MutationPolicyInput {
                    resolved_item: Resolution::Missing,
                    ..input(10)
                }
            ),
            MutationPolicyOutcome::MissingResolution
        );
        assert_eq!(
            evaluate_mutation(
                Some(&plan),
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
        let unsupported = MovePlan {
            source: ItemLocation::Other("unknown".into()),
            ..plan
        };
        assert_eq!(
            evaluate_mutation(Some(&unsupported), &input(10)),
            MutationPolicyOutcome::UnsupportedLocation
        );
    }
}
