//! Combined-state batch equip: one fresh preflight and one EquipItems attempt.
use crate::MutationService;
use crate::{ItemSlot, MutationError};
use destiny_core::{
    DefinitionRepository, DestinationSelector, DestinyMembership, EquipItemsGateway,
    EquipItemsOutcome, EquipItemsRequest, EquipPlan, EquipRequest, FreshnessPolicy,
    InventoryRepository, ItemInstanceId, ItemLocation, ItemSelector, MoveIntentAuthorization,
    PlanId, PlanIdGenerator, ProfileGateway, ProfileSyncReason, Resolution, SearchRepository,
    SearchTerm, StepAction, StepOutcome, Timestamp, TransferGateway, TransferRequest,
};

/// Monotonic spacing shared by production CLI and MCP compositions.
pub fn pace(last: &mut Option<std::time::Instant>) {
    if let Some(previous) = last
        && let Some(wait) = std::time::Duration::from_millis(100).checked_sub(previous.elapsed())
    {
        std::thread::sleep(wait);
    }
    *last = Some(std::time::Instant::now());
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchEquipRequest {
    pub character: String,
    pub items: Vec<String>,
    pub intent: MoveIntentAuthorization,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchItemStatus {
    NoOp,
    Success,
    Failure(i32),
    NotReported,
    NotAttempted,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchEquipItem {
    pub item: ItemInstanceId,
    pub name: String,
    pub slot: ItemSlot,
    pub status: BatchItemStatus,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchEquipResult {
    pub items: Vec<BatchEquipItem>,
    pub receipt: Option<EquipItemsOutcome>,
    pub local_state_updated: bool,
    pub transfer_failed: bool,
}
impl BatchEquipResult {
    pub fn applied(&self) -> bool {
        self.local_state_updated
            && !self.transfer_failed
            && !self.items.is_empty()
            && self.items.iter().all(|item| {
                matches!(
                    item.status,
                    BatchItemStatus::NoOp | BatchItemStatus::Success
                )
            })
    }
    pub fn no_op(&self) -> bool {
        self.applied()
            && self
                .items
                .iter()
                .all(|item| item.status == BatchItemStatus::NoOp)
    }
}

impl<G, R, P> MutationService<G, R, P>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    P: PlanIdGenerator,
{
    /// `pace` runs before each action; production callers enforce 100ms spacing.
    /// All requests are validated before the first dispatch, with no retry.
    pub fn equip_items<T: TransferGateway, E: EquipItemsGateway + ?Sized>(
        &mut self,
        gateways: (&mut T, &mut E),
        request: &BatchEquipRequest,
        policy: FreshnessPolicy,
        now: Timestamp,
        mut pace: impl FnMut(),
    ) -> Result<BatchEquipResult, MutationError> {
        if !request.intent.is_explicit() {
            return Err(MutationError::MissingIntent);
        }
        if !(2..=destiny_core::MAX_EQUIP_ITEMS).contains(&request.items.len()) {
            return Err(MutationError::InvalidPlan);
        }
        SearchTerm::new(&request.character).map_err(MutationError::from)?;
        for item in &request.items {
            SearchTerm::new(item).map_err(MutationError::from)?;
        }
        self.sync_mut()
            .sync(ProfileSyncReason::Preflight, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self
            .repository()
            .load_current()
            .map_err(MutationError::from)?
            .ok_or(MutationError::InvalidProjection)?;
        let exact: Vec<_> = snapshot
            .characters
            .iter()
            .filter(|c| c.id.as_str() == request.character)
            .collect();
        let matches = if exact.is_empty() {
            snapshot
                .characters
                .iter()
                .filter(|c| {
                    c.class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(&request.character))
                })
                .collect()
        } else {
            exact
        };
        let character = match matches.as_slice() {
            [character] => *character,
            [] => return Err(MutationError::MissingDestination),
            _ => return Err(MutationError::AmbiguousDestination),
        };
        let destination = ItemLocation::Character(character.id.clone());
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id.clone(),
        )
        .map_err(MutationError::from)?;
        let mut equip_effects = Vec::new();
        let mut resolved = Vec::new();
        let mut slots = Vec::new();
        let mut buckets = Vec::new();
        let mut items = Vec::new();
        for raw in &request.items {
            let identity = ItemInstanceId::new(raw);
            let selector = match self
                .repository()
                .lookup_instance(&identity)
                .map_err(MutationError::from)?
            {
                Resolution::Unique(_) | Resolution::Ambiguous(_) => {
                    ItemSelector::instance(identity)
                }
                Resolution::Missing if raw.bytes().all(|b| b.is_ascii_digit()) => {
                    ItemSelector::instance(identity)
                }
                Resolution::Missing => ItemSelector::semantic(raw).map_err(MutationError::from)?,
            };
            let equip_request = EquipRequest {
                membership: snapshot.metadata.membership_id.clone(),
                item: selector,
                destination: DestinationSelector::character(character.id.clone()),
                intent: request.intent,
            };
            let item = self.resolve_item(&equip_request.as_move_request())?;
            if resolved
                .iter()
                .any(|i: &destiny_core::ItemInstance| i.instance_id == item.instance_id)
            {
                return Err(MutationError::InvalidPlan);
            }
            let definition = self.equip_definition(item.item_hash)?;
            let slot = definition.slot.ok_or(MutationError::NotEquippable)?;
            if slots.contains(&slot)
                || definition
                    .bucket_hash
                    .is_some_and(|bucket| buckets.contains(&bucket))
            {
                return Err(MutationError::InvalidPlan);
            }
            slots.push(slot);
            if let Some(bucket) = definition.bucket_hash {
                buckets.push(bucket);
            }
            items.push(BatchEquipItem {
                item: item.instance_id.clone(),
                name: definition.name,
                slot,
                status: if item.equipped && item.location == destination {
                    BatchItemStatus::NoOp
                } else {
                    BatchItemStatus::NotAttempted
                },
            });
            equip_effects.push(self.equip_effect(&item, &character.id, &snapshot)?);
            resolved.push(item);
        }
        // Remove equipped items in replaced slots, then insert the intended set.
        // Reuse single-item safety against this combined final equipment state.
        let mut combined = snapshot.clone();
        let mut kept = Vec::new();
        for item in &snapshot.items {
            if item.equipped && item.location == destination {
                let definition = self.equip_definition(item.item_hash)?;
                if definition.slot.is_some_and(|slot| slots.contains(&slot)) {
                    continue;
                }
            }
            if !resolved
                .iter()
                .any(|selected| selected.instance_id == item.instance_id)
            {
                kept.push(item.clone());
            }
        }
        for item in &resolved {
            let mut equipped = item.clone();
            equipped.location = destination.clone();
            equipped.equipped = true;
            kept.push(equipped);
        }
        combined.items = kept;
        for item in &resolved {
            self.validate_equip(item, &destination, &combined)?;
        }
        for item in combined
            .items
            .iter()
            .filter(|item| item.equipped && item.location == destination)
        {
            let definition = self.equip_definition(item.item_hash)?;
            if definition
                .slot
                .is_some_and(crate::mutation::is_equipment_slot)
            {
                self.validate_equip(item, &destination, &combined)?;
            }
        }
        let current = self
            .repository()
            .load_current()
            .map_err(MutationError::from)?
            .ok_or(MutationError::InvalidProjection)?;
        if current.metadata.membership_id != snapshot.metadata.membership_id
            || current.metadata.membership_type != snapshot.metadata.membership_type
        {
            return Err(MutationError::WrongMembership);
        }
        if current.metadata.revision != snapshot.metadata.revision {
            return Err(MutationError::StaleRevision);
        }
        let pending: Vec<_> = items
            .iter()
            .filter(|i| i.status != BatchItemStatus::NoOp)
            .map(|i| i.item.clone())
            .collect();
        let mut result = BatchEquipResult {
            items,
            receipt: None,
            local_state_updated: true,
            transfer_failed: false,
        };
        if pending.is_empty() {
            return Ok(result);
        }
        let provider = EquipItemsRequest::new(membership.clone(), character.id.clone(), pending)
            .map_err(MutationError::from)?;
        let mut transfers = Vec::new();
        for item in &resolved {
            if item.equipped && item.location == destination {
                continue;
            }
            let plan = EquipPlan::new(
                PlanId::new("batch-equip"),
                snapshot.metadata.membership_id.clone(),
                item.instance_id.clone(),
                item.location.clone(),
                character.id.clone(),
                false,
                snapshot.metadata.revision,
                now,
                destiny_core::PlanLifetime::MAX
                    .expires_at(now)
                    .map_err(MutationError::from)?,
            )
            .map_err(|_| MutationError::InvalidPlan)?;
            for step in plan.steps() {
                if let StepAction::Transfer { from, to } = &step.action {
                    transfers.push(
                        TransferRequest::new(
                            membership.clone(),
                            item.instance_id.clone(),
                            item.item_hash,
                            item.quantity,
                            from.clone(),
                            to.clone(),
                        )
                        .map_err(MutationError::from)?,
                    );
                }
            }
        }
        let mut dispatched = false;
        let mut effects = Vec::new();
        for transfer in transfers {
            pace();
            let outcome = gateways.0.transfer(&transfer);
            if outcome.is_ok() {
                dispatched = true;
            }
            if matches!(outcome, Ok(destiny_core::TransferOutcome::Transferred)) {
                effects.push(destiny_core::InventoryEffect::Relocate {
                    item: transfer.item().clone(),
                    to: transfer.to().clone(),
                });
            }
            if !outcome.is_ok_and(|o| StepOutcome::from_transfer(o).continues()) {
                result.transfer_failed = true;
                break;
            }
        }
        if !result.transfer_failed {
            pace();
            match gateways.1.equip_items(&provider) {
                Ok(receipt) => {
                    dispatched = true;
                    for item in &mut result.items {
                        if item.status == BatchItemStatus::NoOp {
                            continue;
                        }
                        item.status = match &receipt {
                            EquipItemsOutcome::Acknowledged(results) => match results
                                .iter()
                                .find(|r| r.item == item.item)
                                .map(|r| r.status)
                            {
                                Some(destiny_core::EquipItemStatus::Success) => {
                                    BatchItemStatus::Success
                                }
                                Some(destiny_core::EquipItemStatus::Failure(code)) => {
                                    BatchItemStatus::Failure(code)
                                }
                                _ => BatchItemStatus::NotReported,
                            },
                            _ => BatchItemStatus::NotReported,
                        };
                    }
                    result.receipt = Some(receipt);
                }
                Err(_) => result.transfer_failed = true,
            }
        }
        for (item, effect) in result.items.iter().zip(equip_effects) {
            if item.status == BatchItemStatus::Success {
                effects.push(effect);
            }
        }
        if dispatched {
            result.local_state_updated = self
                .persist_attempt_effects(&membership, &effects, result.applied())
                .is_ok();
        }
        Ok(result)
    }
}
