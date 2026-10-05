//! Stable equip JSON projection. Contains no transport or process-exit policy.
use destiny_app::{
    Character, EquipRejection, ExecutionEvidence, ItemInstance, ItemLocation, MoveExecutionOutcome,
    MutationError, StepAction, StepOutcome, TransferRejection, VerificationFailureReason,
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipStatus {
    VerifiedSuccess,
    VerifiedNoop,
    Incomplete,
    VerificationFailed,
    AppliedPendingProviderRefresh,
    Refused,
}
impl EquipStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VerifiedSuccess => "verified_success",
            Self::VerifiedNoop => "verified_noop",
            Self::Incomplete => "incomplete",
            Self::VerificationFailed => "verification_failed",
            Self::AppliedPendingProviderRefresh => "applied_pending_provider_refresh",
            Self::Refused => "refused",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRefresh {
    NotRequired,
    Refreshed,
    Failed,
}
#[derive(Clone, Debug)]
pub struct EquipItem {
    pub name: String,
    pub instance_id: String,
    pub slot: Option<String>,
}
#[derive(Clone, Debug)]
pub struct EquipCharacter {
    pub character_id: String,
    pub label: String,
}
#[derive(Clone, Debug)]
pub struct EquipLocation {
    pub kind: &'static str,
    pub character_id: Option<String>,
    pub label: String,
}
impl EquipLocation {
    fn json(&self) -> Value {
        let mut value = json!({"kind":self.kind, "label":self.label});
        if let Some(id) = &self.character_id {
            value["character_id"] = json!(id);
        }
        value
    }
}
#[derive(Clone, Debug)]
pub struct EquipStep {
    pub ordinal: u8,
    pub kind: &'static str,
    pub from: Option<EquipLocation>,
    pub to: EquipLocation,
    pub outcome: &'static str,
    pub reason: Option<String>,
}
#[derive(Clone, Debug)]
pub struct EquipObservation {
    pub location: EquipLocation,
    pub equipped: bool,
}
#[derive(Clone, Debug)]
pub struct EquipOutput {
    pub status: EquipStatus,
    pub item: Option<EquipItem>,
    pub character: Option<EquipCharacter>,
    pub steps: Vec<EquipStep>,
    pub reason: Option<String>,
    pub profile_refresh: ProfileRefresh,
    pub observed: Option<EquipObservation>,
}
pub fn location_view(location: &ItemLocation, characters: &[Character]) -> EquipLocation {
    match location {
        ItemLocation::Vault => EquipLocation {
            kind: "vault",
            character_id: None,
            label: "Vault".into(),
        },
        ItemLocation::Character(id) => EquipLocation {
            kind: "character",
            character_id: Some(id.as_str().into()),
            label: characters
                .iter()
                .find(|c| c.id == *id)
                .and_then(|c| c.class.as_deref().or(c.name.as_deref()))
                .unwrap_or(id.as_str())
                .into(),
        },
        _ => EquipLocation {
            kind: "unknown",
            character_id: None,
            label: "Unknown".into(),
        },
    }
}
pub fn mutation_reason(error: MutationError) -> String {
    match error {
        MutationError::AmbiguousItem | MutationError::AmbiguousResolution => "item selection is ambiguous; use the instance id (from search-inventory) to disambiguate".into(),
        MutationError::AmbiguousDestination => "character selection is ambiguous; use the character id (from list-characters) to disambiguate".into(),
        MutationError::MissingItem | MutationError::MissingResolution => "item was not found".into(),
        MutationError::MissingDestination => "character was not found".into(),
        _ => error.to_string(),
    }
}
pub fn step_view(
    ordinal: u8,
    action: &StepAction,
    outcome: StepOutcome,
    characters: &[Character],
) -> EquipStep {
    let (kind, from, to) = match action {
        StepAction::Transfer { from, to } => (
            "transfer",
            Some(location_view(from, characters)),
            location_view(to, characters),
        ),
        StepAction::Equip { character } => (
            "equip",
            None,
            location_view(&ItemLocation::Character(character.clone()), characters),
        ),
    };
    let (outcome, reason) = match outcome {
        StepOutcome::AuthenticationRefused => (
            "authentication_refused",
            Some("provider authentication failed"),
        ),
        StepOutcome::Completed => ("completed", None),
        StepOutcome::Unattempted => ("not_run", None),
        StepOutcome::Uncertain => (
            "uncertain",
            Some("provider outcome is uncertain; do not blindly retry"),
        ),
        StepOutcome::Transient => (
            "failed",
            Some("provider temporarily unavailable; step did not apply"),
        ),
        StepOutcome::Rejected(reason) => (
            "rejected",
            Some(match reason {
                TransferRejection::NotTransferable => "item is not transferable",
                TransferRejection::Equipped => "item is equipped; it must be unequipped first",
                TransferRejection::DestinationFull => "destination has no room for this item",
                TransferRejection::InvalidRequest => "transfer request was rejected",
            }),
        ),
        StepOutcome::EquipRejected(reason) => (
            "rejected",
            Some(match reason {
                EquipRejection::NotInSocialSpace => {
                    "character must be in a social space, orbit, or offline"
                }
                EquipRejection::ExoticLimit => "equipping this item would exceed the exotic limit",
                EquipRejection::Unequippable => "item is not equippable on this character",
                EquipRejection::ItemNotFound => "item was not found",
                EquipRejection::CharacterNotFound => "character was not found",
                EquipRejection::InvalidRequest => "equip request was rejected",
            }),
        ),
    };
    EquipStep {
        ordinal,
        kind,
        from,
        to,
        outcome,
        reason: reason.map(str::to_owned),
    }
}
impl EquipOutput {
    pub fn refused(error: MutationError) -> Self {
        Self {
            status: EquipStatus::Refused,
            item: None,
            character: None,
            steps: vec![],
            reason: Some(mutation_reason(error)),
            profile_refresh: ProfileRefresh::NotRequired,
            observed: None,
        }
    }
    pub fn with_evidence(mut self, evidence: &ExecutionEvidence, characters: &[Character]) -> Self {
        self.steps = evidence
            .steps()
            .iter()
            .map(|r| step_view(r.step.ordinal, &r.step.action, r.outcome, characters))
            .collect();
        self
    }
    pub fn from_outcome(
        outcome: &MoveExecutionOutcome,
        characters: &[Character],
        observed: Option<&ItemInstance>,
    ) -> Self {
        let (status, evidence, refresh, reason) = match outcome {
            MoveExecutionOutcome::VerifiedSuccess(e) => (
                EquipStatus::VerifiedSuccess,
                e,
                ProfileRefresh::Refreshed,
                None,
            ),
            MoveExecutionOutcome::VerifiedNoOp(e) => (
                EquipStatus::VerifiedNoop,
                e,
                ProfileRefresh::NotRequired,
                None,
            ),
            MoveExecutionOutcome::AppliedPendingProviderRefresh(e) => (
                EquipStatus::AppliedPendingProviderRefresh,
                e.evidence(),
                ProfileRefresh::Refreshed,
                Some("the provider confirmed every step, but its profile still shows the pre-action state; observed state is stale/unconfirmed; do not retry, run `ghostlink sync` later to confirm".into()),
            ),
            MoveExecutionOutcome::IncompleteExecution(e) => (
                EquipStatus::Incomplete,
                e.evidence(),
                ProfileRefresh::Refreshed,
                e.error().map(mutation_reason),
            ),
            MoveExecutionOutcome::VerificationFailed(e) => (
                EquipStatus::VerificationFailed,
                e.evidence(),
                if e.reason() == VerificationFailureReason::RefreshFailed {
                    ProfileRefresh::Failed
                } else {
                    ProfileRefresh::Refreshed
                },
                Some(match e.reason() {
                    VerificationFailureReason::RefreshFailed => {
                        "outcome is uncertain: post-action profile refresh failed".into()
                    }
                    VerificationFailureReason::Unresolved => {
                        "outcome is uncertain: refreshed state could not uniquely resolve the item"
                            .into()
                    }
                    VerificationFailureReason::Elsewhere => {
                        "refreshed state contradicts the reported progress; equip was not verified"
                            .into()
                    }
                }),
            ),
        };
        let observation = if refresh == ProfileRefresh::Failed {
            None
        } else {
            observed.map(|i| EquipObservation {
                location: location_view(&i.location, characters),
                equipped: i.equipped,
            })
        };
        Self {
            status,
            item: None,
            character: None,
            steps: vec![],
            reason,
            profile_refresh: refresh,
            observed: observation,
        }
        .with_evidence(evidence, characters)
    }
    pub fn json(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "item": self.item.as_ref().map(|i| json!({"name":i.name,"instance_id":i.instance_id,"slot":i.slot})),
            "character": self.character.as_ref().map(|c| json!({"character_id":c.character_id,"label":c.label})),
            "steps": self.steps.iter().map(|s| {
                let mut v = json!({"ordinal":s.ordinal,"kind":s.kind,"to":s.to.json(),"outcome":s.outcome});
                if let Some(from) = &s.from { v["from"] = from.json(); }
                if let Some(reason) = &s.reason { v["reason"] = json!(reason); }
                v
            }).collect::<Vec<_>>(),
            "reason":self.reason,
            "profile_refresh":match self.profile_refresh {ProfileRefresh::NotRequired=>"not_required",ProfileRefresh::Refreshed=>"refreshed",ProfileRefresh::Failed=>"failed"},
            "observed":self.observed.as_ref().map(|o|json!({"location":o.location.json(),"equipped":o.equipped})),
        })
    }
}
