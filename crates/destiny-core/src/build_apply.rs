//! Pure, optimistic item-phase planning. No provider calls or inventory writes.
//! DIM loadout-apply.ts:355-660; item-move-service.ts:216-257,570-741,1220-1279.
use crate::{build_analysis::ResolvedBuildItem, domain::ClassType, *};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyPhase {
    Dequip,
    Move,
    Equip,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildAction {
    Move {
        item: ItemInstanceId,
        from: ItemLocation,
        to: ItemLocation,
    },
    Equip {
        items: Vec<ItemInstanceId>,
        character: CharacterId,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildApplyStep {
    pub phase: ApplyPhase,
    pub action: BuildAction,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplySkipReason {
    Unresolved,
    MissingDefinition,
    UnsupportedBucket,
    ClassMismatch,
    NotTransferable,
    NoReplacement,
    NoSpace,
    VaultCapacityUnavailable,
    OccupancyUnknown,
    UniqueConflict,
    UnsupportedLocation,
    DuplicateSlot,
}
impl ApplySkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::MissingDefinition => "missing_definition",
            Self::UnsupportedBucket => "unsupported_bucket",
            Self::ClassMismatch => "class_mismatch",
            Self::NotTransferable => "not_transferable",
            Self::NoReplacement => "no_replacement",
            Self::NoSpace => "no_space",
            Self::VaultCapacityUnavailable => "vault_capacity_unavailable",
            Self::OccupancyUnknown => "occupancy_unknown",
            Self::UniqueConflict => "unique_conflict",
            Self::UnsupportedLocation => "unsupported_location",
            Self::DuplicateSlot => "duplicate_slot",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildApplySkip {
    pub spec_index: usize,
    pub reason: ApplySkipReason,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildApplyPlan {
    pub target: CharacterId,
    pub steps: Vec<BuildApplyStep>,
    pub skipped: Vec<BuildApplySkip>,
    pub already_there: Vec<usize>,
    pub not_applied: Vec<&'static str>,
}
struct Planner<'a> {
    items: Vec<ItemInstance>,
    defs: &'a [ItemDefinition],
    characters: &'a [Character],
    policy: &'a EquipmentCapacityPolicy,
    vault: Option<usize>,
    excluded: Vec<ItemInstanceId>,
    active: Vec<ItemInstanceId>,
    steps: Vec<BuildApplyStep>,
    phase: ApplyPhase,
}
impl Planner<'_> {
    fn definition(&self, item: &ItemInstance) -> Result<&ItemDefinition, ApplySkipReason> {
        self.defs
            .iter()
            .find(|d| d.hash == item.item_hash)
            .ok_or(ApplySkipReason::MissingDefinition)
    }
    fn item(&self, id: &ItemInstanceId) -> ItemInstance {
        self.items
            .iter()
            .find(|i| &i.instance_id == id)
            .expect("planner-owned identity")
            .clone()
    }
    fn bucket(&self, item: &ItemInstance) -> Result<BucketHash, ApplySkipReason> {
        self.definition(item)?
            .bucket_hash
            .filter(|b| self.policy.capacity(*b).is_some())
            .ok_or(ApplySkipReason::UnsupportedBucket)
    }
    fn label(&self, item: &ItemInstance) -> Option<&str> {
        self.definition(item)
            .ok()?
            .armor
            .unique_label
            .as_deref()
            .filter(|s| !s.is_empty())
    }
    fn usable(&self, item: &ItemInstance, target: &CharacterId) -> bool {
        let Some(character) = self.characters.iter().find(|c| &c.id == target) else {
            return false;
        };
        let Ok(def) = self.definition(item) else {
            return false;
        };
        let class = character
            .class
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        matches!(def.class_type, Some(ClassType::Any))
            || matches!(
                (def.class_type, class.as_str()),
                (Some(ClassType::Hunter), "hunter")
                    | (Some(ClassType::Titan), "titan")
                    | (Some(ClassType::Warlock), "warlock")
            )
    }
    fn rarity(&self, item: &ItemInstance) -> u8 {
        match self.definition(item).ok().and_then(|d| d.tier.as_deref()) {
            Some("Exotic") => 6,
            Some("Legendary") => 5,
            Some("Rare") => 4,
            Some("Uncommon") => 3,
            Some("Common") => 2,
            _ => 0,
        }
    }
    fn replacement(
        &self,
        item: &ItemInstance,
        owner: &CharacterId,
    ) -> Result<ItemInstanceId, ApplySkipReason> {
        let bucket = self.bucket(item)?;
        let mut candidates: Vec<_> = self
            .items
            .iter()
            .filter(|i| {
                i.instance_id != item.instance_id
                    && !i.equipped
                    && !self.excluded.contains(&i.instance_id)
                    && !self.active.contains(&i.instance_id)
                    && self.bucket(i) == Ok(bucket)
                    && self.usable(i, owner)
                    && matches!(i.location, ItemLocation::Character(_) | ItemLocation::Vault)
                    && (i.location == ItemLocation::Character(owner.clone())
                        || self.definition(i).is_ok_and(|d| !i.notransfer(d)))
                    && (self.label(item).is_none() || self.label(i) != self.label(item))
                    && self.label(i).is_none_or(|label| {
                        !self.items.iter().any(|other| {
                            other.equipped
                                && other.location == ItemLocation::Character(owner.clone())
                                && other.instance_id != item.instance_id
                                && self.label(other) == Some(label)
                                && self.bucket(other) != Ok(bucket)
                        })
                    })
            })
            .collect();
        // Stable ties; Ghostlink has no DIM user tags or manual-move timestamps.
        candidates.sort_by_key(|i| {
            let store = if i.location == ItemLocation::Character(owner.clone()) {
                0
            } else if i.location == ItemLocation::Vault {
                1
            } else {
                2
            };
            (
                store,
                self.label(i).is_some(),
                std::cmp::Reverse(
                    self.definition(i).ok().and_then(|d| d.item_type.as_deref())
                        == self
                            .definition(item)
                            .ok()
                            .and_then(|d| d.item_type.as_deref()),
                ),
                std::cmp::Reverse(self.rarity(i)),
                std::cmp::Reverse(i.power.unwrap_or(0)),
            )
        });
        candidates
            .first()
            .map(|i| i.instance_id.clone())
            .ok_or(ApplySkipReason::NoReplacement)
    }
    fn equip(
        &mut self,
        ids: Vec<ItemInstanceId>,
        character: &CharacterId,
    ) -> Result<(), ApplySkipReason> {
        for id in &ids {
            let item = self.item(id);
            if !self.usable(&item, character) {
                return Err(ApplySkipReason::ClassMismatch);
            }
            if item.location != ItemLocation::Character(character.clone()) {
                return Err(ApplySkipReason::UnsupportedLocation);
            }
        }
        for id in &ids {
            let bucket = self.bucket(&self.item(id))?;
            let replaced: Vec<_> = self
                .items
                .iter()
                .filter(|i| {
                    i.location == ItemLocation::Character(character.clone())
                        && self.bucket(i) == Ok(bucket)
                })
                .map(|i| i.instance_id.clone())
                .collect();
            for i in &mut self.items {
                if replaced.contains(&i.instance_id) {
                    i.equipped = &i.instance_id == id;
                }
            }
        }
        self.steps.push(BuildApplyStep {
            phase: self.phase,
            action: BuildAction::Equip {
                items: ids,
                character: character.clone(),
            },
        });
        Ok(())
    }
    fn dequip(&mut self, id: &ItemInstanceId) -> Result<(), ApplySkipReason> {
        let item = self.item(id);
        if !item.equipped {
            return Ok(());
        }
        let ItemLocation::Character(owner) = &item.location else {
            return Err(ApplySkipReason::UnsupportedLocation);
        };
        let replacement = self.replacement(&item, owner)?;
        self.move_item(&replacement, &item.location)?;
        self.equip(vec![replacement], owner)
    }
    fn free(&self, target: &ItemLocation, bucket: BucketHash) -> Result<usize, ApplySkipReason> {
        let capacity = match target {
            ItemLocation::Vault => self
                .vault
                .ok_or(ApplySkipReason::VaultCapacityUnavailable)?,
            ItemLocation::Character(_) => self
                .policy
                .capacity(bucket)
                .ok_or(ApplySkipReason::UnsupportedBucket)?,
            _ => return Err(ApplySkipReason::UnsupportedLocation),
        };
        let mut occupied = 0;
        for i in self.items.iter().filter(|i| &i.location == target) {
            let b = self
                .bucket(i)
                .map_err(|_| ApplySkipReason::OccupancyUnknown)?;
            if *target == ItemLocation::Vault || b == bucket {
                occupied += 1;
            }
        }
        Ok(capacity.saturating_sub(occupied))
    }
    fn space(
        &mut self,
        target: &ItemLocation,
        incoming: &ItemInstance,
    ) -> Result<(), ApplySkipReason> {
        let bucket = self.bucket(incoming)?;
        if self.free(target, bucket)? > 0 {
            return Ok(());
        }
        let mut candidates: Vec<_> = self
            .items
            .iter()
            .filter(|i| {
                &i.location == target
                    && !self.excluded.contains(&i.instance_id)
                    && !self.active.contains(&i.instance_id)
                    && self.definition(i).is_ok_and(|d| !i.notransfer(d))
                    && (*target == ItemLocation::Vault || self.bucket(i) == Ok(bucket))
            })
            .cloned()
            .collect();
        let mut characters: Vec<_> = self
            .characters
            .iter()
            .filter(|c| *target != ItemLocation::Character(c.id.clone()))
            .collect();
        characters.sort_by_key(|c| c.date_last_played.map_or(0, |t| t.seconds()));
        for character in characters {
            candidates.sort_by_key(|i| {
                (
                    i.equipped,
                    std::cmp::Reverse(self.usable(i, &character.id)),
                    if *target == ItemLocation::Vault {
                        std::cmp::Reverse(self.rarity(i))
                    } else {
                        std::cmp::Reverse(6 - self.rarity(i))
                    },
                    if *target == ItemLocation::Vault {
                        std::cmp::Reverse(i.power.unwrap_or(0))
                    } else {
                        std::cmp::Reverse(u16::MAX - i.power.unwrap_or(0))
                    },
                )
            });
            for candidate in &candidates {
                let destination = ItemLocation::Character(character.id.clone());
                if self.free(&destination, self.bucket(candidate)?)? > 0
                    && (*target == ItemLocation::Vault
                        || self.free(&ItemLocation::Vault, self.bucket(candidate)?)? == 1)
                {
                    return self.move_item(&candidate.instance_id, &destination);
                }
            }
        }
        if *target != ItemLocation::Vault
            && let Some(candidate) = candidates.first()
        {
            return self.move_item(&candidate.instance_id, &ItemLocation::Vault);
        }
        Err(ApplySkipReason::NoSpace)
    }
    fn move_item(
        &mut self,
        id: &ItemInstanceId,
        target: &ItemLocation,
    ) -> Result<(), ApplySkipReason> {
        let item = self.item(id);
        if &item.location == target {
            return Ok(());
        }
        self.bucket(&item)?;
        if item.notransfer(self.definition(&item)?) {
            return Err(ApplySkipReason::NotTransferable);
        }
        if !matches!(
            item.location,
            ItemLocation::Character(_) | ItemLocation::Vault
        ) {
            return Err(ApplySkipReason::UnsupportedLocation);
        }
        if self.active.contains(id) {
            return Err(ApplySkipReason::NoSpace);
        }
        self.active.push(id.clone());
        let result = (|| {
            self.dequip(id)?;
            if item.location != ItemLocation::Vault && *target != ItemLocation::Vault {
                self.leg(id, &ItemLocation::Vault)?;
            }
            self.leg(id, target)
        })();
        self.active.pop();
        result
    }
    fn leg(&mut self, id: &ItemInstanceId, target: &ItemLocation) -> Result<(), ApplySkipReason> {
        let item = self.item(id);
        self.space(target, &item)?;
        self.steps.push(BuildApplyStep {
            phase: self.phase,
            action: BuildAction::Move {
                item: id.clone(),
                from: item.location,
                to: target.clone(),
            },
        });
        let changed = self
            .items
            .iter_mut()
            .find(|i| &i.instance_id == id)
            .expect("planner-owned identity");
        changed.location = target.clone();
        changed.equipped = false;
        Ok(())
    }
    fn attempt(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), ApplySkipReason>,
    ) -> Result<(), ApplySkipReason> {
        let items = self.items.clone();
        let length = self.steps.len();
        let result = f(self);
        if result.is_err() {
            self.items = items;
            self.steps.truncate(length);
        }
        result
    }
}
/// Vault capacity is explicit evidence, never inferred from character capacities.
/// The current production projection cannot supply it; previews report that gap.
pub fn plan_build_items(
    target: &CharacterId,
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    characters: &[Character],
    policy: &EquipmentCapacityPolicy,
    vault_capacity: Option<usize>,
) -> BuildApplyPlan {
    let mut p = Planner {
        items: inventory.to_vec(),
        defs,
        characters,
        policy,
        vault: vault_capacity,
        excluded: selected
            .iter()
            .filter_map(|s| s.inventory.as_ref().map(|i| i.instance_id.clone()))
            .collect(),
        active: vec![],
        steps: vec![],
        phase: ApplyPhase::Dequip,
    };
    let mut plan = BuildApplyPlan {
        target: target.clone(),
        steps: vec![],
        skipped: vec![],
        already_there: vec![],
        not_applied: vec!["sockets_mods_subclass_deferred_to_be_007b"],
    };
    let mut order: Vec<_> = selected.iter().enumerate().collect();
    let buckets = [
        1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
    ];
    order.sort_by_key(|(_, s)| {
        s.inventory
            .as_ref()
            .and_then(|i| p.bucket(i).ok())
            .and_then(|b| buckets.iter().position(|h| *h == b.value()))
            .unwrap_or(usize::MAX)
    });
    let destination = ItemLocation::Character(target.clone());
    let mut pending = vec![];
    let mut slots = vec![];
    let mut labels = vec![];
    for (index, s) in order {
        let validation = (|| {
            let item = s.inventory.as_ref().ok_or(ApplySkipReason::Unresolved)?;
            let bucket = p.bucket(item)?;
            if !p.items.iter().any(|i| i.instance_id == item.instance_id) {
                return Err(ApplySkipReason::Unresolved);
            }
            if item.location != destination && item.notransfer(p.definition(item)?) {
                return Err(ApplySkipReason::NotTransferable);
            }
            if item.location != destination
                && matches!(item.location, ItemLocation::Character(_))
                && vault_capacity.is_none()
            {
                return Err(ApplySkipReason::VaultCapacityUnavailable);
            }
            if s.should_equip {
                if !p.usable(item, target) {
                    return Err(ApplySkipReason::ClassMismatch);
                }
                if slots.contains(&bucket) {
                    return Err(ApplySkipReason::DuplicateSlot);
                }
                if p.label(item)
                    .is_some_and(|l| labels.contains(&l.to_string()))
                {
                    return Err(ApplySkipReason::UniqueConflict);
                }
            }
            Ok((item.instance_id.clone(), bucket))
        })();
        match validation {
            Err(reason) => plan.skipped.push(BuildApplySkip {
                spec_index: index,
                reason,
            }),
            Ok((id, bucket)) => {
                let result = p.attempt(|p| {
                    let item = p.item(&id);
                    if item.location != destination {
                        p.dequip(&id)?;
                    }
                    Ok(())
                });
                if let Err(reason) = result {
                    plan.skipped.push(BuildApplySkip {
                        spec_index: index,
                        reason,
                    });
                    continue;
                }
                if s.should_equip {
                    slots.push(bucket);
                    if let Some(label) = p.label(&p.item(&id)) {
                        labels.push(label.to_string());
                    }
                }
                pending.push((index, id, s.should_equip));
            }
        }
    }
    p.phase = ApplyPhase::Move;
    let mut equips = vec![];
    for (index, id, equip) in pending {
        let item = p.item(&id);
        if item.location == destination && (!equip || item.equipped) {
            plan.already_there.push(index);
            continue;
        }
        match p.attempt(|p| p.move_item(&id, &destination)) {
            Err(reason) => plan.skipped.push(BuildApplySkip {
                spec_index: index,
                reason,
            }),
            Ok(()) => {
                if equip {
                    equips.push((index, id));
                }
            }
        }
    }
    p.phase = ApplyPhase::Equip;
    if !equips.is_empty() {
        let result = p.attempt(|p| {
            let mut ids: Vec<_> = equips.iter().map(|(_, id)| id.clone()).collect();
            for (_, id) in &equips {
                let item = p.item(id);
                let bucket = p.bucket(&item)?;
                let conflicts: Vec<_> = p
                    .items
                    .iter()
                    .filter(|other| {
                        other.equipped
                            && other.location == destination
                            && p.label(&item).is_some()
                            && p.label(other) == p.label(&item)
                            && p.bucket(other) != Ok(bucket)
                            && !ids
                                .iter()
                                .any(|id| p.bucket(&p.item(id)) == p.bucket(other))
                    })
                    .cloned()
                    .collect();
                for other in conflicts {
                    let replacement = p.replacement(&other, target)?;
                    p.move_item(&replacement, &destination)?;
                    ids.push(replacement);
                }
            }
            p.equip(ids, target)
        });
        if let Err(reason) = result {
            for (index, _) in equips {
                plan.skipped.push(BuildApplySkip {
                    spec_index: index,
                    reason,
                });
            }
        }
    }
    plan.skipped.sort_by_key(|s| s.spec_index);
    for step in p.steps {
        if let (Some(previous), BuildAction::Equip { items, character }) =
            (plan.steps.last_mut(), &step.action)
            && previous.phase == ApplyPhase::Dequip
            && step.phase == ApplyPhase::Dequip
            && let BuildAction::Equip {
                items: prior,
                character: owner,
            } = &mut previous.action
            && owner == character
        {
            prior.extend(items.iter().cloned());
            continue;
        }
        plan.steps.push(step);
    }
    plan
}
