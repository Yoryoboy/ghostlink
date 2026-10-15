//! Pure, optimistic item-phase planning. No provider calls or inventory writes.
//! DIM loadout-apply.ts:355-660; item-move-service.ts:216-257,570-741,1220-1279.
use crate::{build_analysis::ResolvedBuildItem, domain::ClassType, *};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyPhase {
    Dequip,
    Move,
    Equip,
    /// Subclass and artifact socket overrides, after equips (`loadout-apply.ts:629-650`).
    Sockets,
    /// Armor mods, after the socket overrides (`loadout-apply.ts:652-672`).
    Mods,
    ClearSpace,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildAction {
    Move {
        item: ItemInstanceId,
        from: ItemLocation,
        to: ItemLocation,
    },
    /// DIM pulls from the owner's Postmaster before any other move.
    PullFromPostmaster {
        item: ItemInstanceId,
        character: CharacterId,
    },
    Equip {
        items: Vec<ItemInstanceId>,
        character: CharacterId,
    },
    /// One free plug insertion (`InsertSocketPlugFree`).
    InsertPlug {
        item: ItemInstanceId,
        character: CharacterId,
        socket_index: u8,
        plug: ItemHash,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildApplyStep {
    pub phase: ApplyPhase,
    pub action: BuildAction,
    /// Earlier steps (indices into the plan) whose success this step relies on:
    /// dequip, Postmaster pull, space-making and earlier legs of the same move.
    pub needs: Vec<usize>,
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
    StackUnsupported,
    PostmasterSideEffects,
    PostmasterSideEffectsUnknown,
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
            Self::StackUnsupported => "stack_unsupported",
            Self::PostmasterSideEffects => "postmaster_side_effects",
            Self::PostmasterSideEffectsUnknown => "postmaster_side_effects_unknown",
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
    /// Final clear-space items that could not be moved off the character.
    pub clear_skipped: Vec<(ItemInstanceId, ApplySkipReason)>,
    /// Socket overrides DIM would refuse or that need evidence Ghostlink lacks.
    pub socket_skipped: Vec<crate::build_sockets::SocketSkip>,
    /// Requested socket overrides already in place, as (spec index, override index).
    pub socket_already_there: Vec<(usize, u16)>,
    /// Requested armor mods that will not be plugged, with the reason.
    pub mod_skipped: Vec<crate::build_mod_apply::ModSkip>,
    /// Requested armor mods already in their socket.
    pub mods_in_place: usize,
    /// What DIM applies that this plan does not, with an explicit reason.
    pub not_applied: Vec<&'static str>,
    /// Ranking or capacity evidence that was not supplied, never guessed.
    pub unknown: Vec<&'static str>,
}
/// DIM Vault correspondence: equipment bucket to its Vault bucket and that
/// bucket's capacity (`stores-helpers.ts:65-66,101-107`, `d2-buckets.ts:60-66`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VaultEvidence {
    pub mapping: Vec<(BucketHash, BucketHash)>,
    pub capacity: Vec<(BucketHash, usize)>,
}
impl VaultEvidence {
    pub fn single(accepted: &[BucketHash], vault: BucketHash, capacity: usize) -> Self {
        Self {
            mapping: accepted.iter().map(|b| (*b, vault)).collect(),
            capacity: vec![(vault, capacity)],
        }
    }
    /// DIM `d2-buckets.ts:59-66`: later `acceptedItems` override earlier ones and a
    /// falsy destination (0) leaves no Vault bucket; a destination without a bucket
    /// definition resolves to nothing. Capacity is that bucket's `itemCount`
    /// (`d2-buckets.ts:47`). No usable mapping is missing evidence, not an empty one.
    pub fn from_catalogs(
        vendor: &crate::armor_catalog::VaultVendorDefinition,
        buckets: &[crate::armor_catalog::InventoryBucketDefinition],
    ) -> Option<Self> {
        let destinations: std::collections::BTreeMap<u32, u32> = vendor
            .accepted_items
            .iter()
            .map(|e| (e.accepted_bucket, e.destination_bucket))
            .collect();
        let mut evidence = Self::default();
        for (accepted, destination) in destinations {
            let vault = BucketHash::new(destination.into());
            let Some(definition) = buckets.iter().find(|b| b.hash == destination) else {
                continue;
            };
            if destination == 0 {
                continue;
            }
            evidence
                .mapping
                .push((BucketHash::new(accepted.into()), vault));
            if !evidence.capacity.iter().any(|(b, _)| *b == vault) {
                evidence.capacity.push((
                    vault,
                    usize::try_from(definition.item_count).unwrap_or(usize::MAX),
                ));
            }
        }
        (!evidence.mapping.is_empty()).then_some(evidence)
    }
    fn vault_bucket(&self, bucket: BucketHash) -> Option<BucketHash> {
        self.mapping.iter().find(|(b, _)| *b == bucket).map(|m| m.1)
    }
}
/// Optional evidence and DIM loadout parameters. Absent evidence is typed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyContext<'a> {
    pub vault: Option<&'a VaultEvidence>,
    /// Saved in-game loadout membership as (owner, item), when known.
    pub loadouts: Option<&'a [(CharacterId, ItemInstanceId)]>,
    pub clear_weapons: bool,
    pub clear_armor: bool,
    /// Socket type, plug set and material requirement records for socket overrides.
    pub catalogs: Option<&'a [crate::armor_catalog::ArmorCatalogRecord]>,
    /// Armor mods, `clearMods` and fashion (`modsByBucket`).
    pub armor_mods: Option<crate::build_mod_apply::ModInput<'a>>,
}
struct Planner<'a> {
    items: Vec<ItemInstance>,
    defs: &'a [ItemDefinition],
    characters: &'a [Character],
    policy: &'a EquipmentCapacityPolicy,
    vault: Option<&'a VaultEvidence>,
    loadouts: Option<&'a [(CharacterId, ItemInstanceId)]>,
    /// DIM clear-space reservations on the target character (item-move-service.ts:775-790).
    reserve: Option<(CharacterId, Vec<(BucketHash, usize)>)>,
    excluded: Vec<ItemInstanceId>,
    active: Vec<ItemInstanceId>,
    steps: Vec<BuildApplyStep>,
    phase: ApplyPhase,
    /// A grouped dequip equip needs every replacement move since this index.
    hard_from: Option<usize>,
    /// Items whose later moves wait for a grouped dequip equip.
    pinned: Vec<(ItemInstanceId, usize)>,
}
impl Planner<'_> {
    /// One plug insertion; it waits for every move, pull or equip of its item.
    fn push_insert(
        &mut self,
        phase: ApplyPhase,
        target: &CharacterId,
        item: ItemInstanceId,
        socket_index: u8,
        plug: ItemHash,
    ) {
        let needs = self
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| step_touches(&s.action, &item))
            .map(|(i, _)| i)
            .collect();
        self.steps.push(BuildApplyStep {
            phase,
            action: BuildAction::InsertPlug {
                item,
                character: target.clone(),
                socket_index,
                plug,
            },
            needs,
        });
    }
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
            .filter(|b| self.policy.capacity(*b).is_some() || is_equip_only_bucket(*b))
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
        taken: &[ItemInstanceId],
    ) -> Result<ItemInstanceId, ApplySkipReason> {
        let bucket = self.bucket(item)?;
        let taken_labels: Vec<_> = taken
            .iter()
            .filter_map(|id| self.label(&self.item(id)))
            .collect();
        let mut candidates: Vec<_> = self
            .items
            .iter()
            .filter(|i| {
                i.instance_id != item.instance_id
                    && !i.equipped
                    && !self.excluded.contains(&i.instance_id)
                    && !self.active.contains(&i.instance_id)
                    && !taken.contains(&i.instance_id)
                    && self.label(i).is_none_or(|l| !taken_labels.contains(&l))
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
        let needs = self
            .hard_from
            .map_or(vec![], |from| (from..self.steps.len()).collect());
        self.steps.push(BuildApplyStep {
            phase: self.phase,
            action: BuildAction::Equip {
                items: ids,
                character: character.clone(),
            },
            needs,
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
        let replacement = self.replacement(&item, owner, &[])?;
        self.move_item(&replacement, &item.location)?;
        self.equip(vec![replacement], owner)
    }
    fn free(&self, target: &ItemLocation, bucket: BucketHash) -> Result<usize, ApplySkipReason> {
        // Equip-only bucket capacity is not audited, so nothing is moved into or
        // out of those buckets; they are only equipped where they already are.
        if self.policy.capacity(bucket).is_none() {
            return Err(ApplySkipReason::UnsupportedBucket);
        }
        let unavailable = ApplySkipReason::VaultCapacityUnavailable;
        let (capacity, vault_bucket) = match (target, self.vault) {
            (ItemLocation::Vault, vault) => {
                let vault = vault.ok_or(unavailable)?;
                let vault_bucket = vault.vault_bucket(bucket).ok_or(unavailable)?;
                let capacity = vault.capacity.iter().find(|(b, _)| *b == vault_bucket);
                (capacity.ok_or(unavailable)?.1, Some((vault, vault_bucket)))
            }
            (ItemLocation::Character(_), _) => (
                self.policy
                    .capacity(bucket)
                    .ok_or(ApplySkipReason::UnsupportedBucket)?,
                None,
            ),
            _ => return Err(ApplySkipReason::UnsupportedLocation),
        };
        let mut occupied = 0;
        for i in self.items.iter().filter(|i| &i.location == target) {
            // Any known bucket counts; a missing definition or bucket is unknown.
            let b = self
                .definition(i)
                .ok()
                .and_then(|d| d.bucket_hash)
                .ok_or(ApplySkipReason::OccupancyUnknown)?;
            match vault_bucket {
                // Vault slots are shared per Vault bucket, not per equipment slot.
                Some((vault, vault_bucket)) => match vault.vault_bucket(b) {
                    Some(found) => occupied += usize::from(found == vault_bucket),
                    None => return Err(ApplySkipReason::OccupancyUnknown),
                },
                None => occupied += usize::from(b == bucket),
            }
        }
        let reserved = match (&self.reserve, target) {
            (Some((owner, buckets)), ItemLocation::Character(id)) if owner == id => buckets
                .iter()
                .find(|(b, _)| *b == bucket)
                .map_or(0, |(_, n)| *n),
            _ => 0,
        };
        Ok(capacity.saturating_sub(occupied).saturating_sub(reserved))
    }
    fn in_loadout(&self, item: &ItemInstance, owner: &CharacterId) -> bool {
        self.loadouts
            .is_some_and(|l| l.iter().any(|(c, i)| c == owner && i == &item.instance_id))
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
        let from = match target {
            ItemLocation::Character(id) => Some(id.clone()),
            _ => None,
        };
        for character in characters {
            // DIM sortMoveAsideCandidatesForStore (item-move-service.ts:1138-1211);
            // true sorts first, tags and manual-move times are neutral.
            candidates.sort_by_key(|i| {
                (
                    i.equipped,
                    std::cmp::Reverse(from.as_ref().is_some_and(|f| !self.in_loadout(i, f))),
                    std::cmp::Reverse(self.in_loadout(i, &character.id)),
                    std::cmp::Reverse(from.as_ref().is_some_and(|f| !self.usable(i, f))),
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
        let mut item = self.item(id);
        if &item.location == target {
            return Ok(());
        }
        // Everything pushed from here on prepares this move (DIM moveToStore).
        let start = self.steps.len();
        self.bucket(&item)?;
        if self.active.contains(id) {
            return Err(ApplySkipReason::NoSpace);
        }
        if let ItemLocation::Postmaster(owner) = item.location.clone() {
            // Pull to the owner first (item-move-service.ts:1072-1083), under the
            // existing Postmaster pull policy (instanced_pull.rs:482-488).
            match self
                .definition(&item)?
                .does_postmaster_pull_have_side_effects
            {
                Some(false) => {}
                Some(true) => return Err(ApplySkipReason::PostmasterSideEffects),
                None => return Err(ApplySkipReason::PostmasterSideEffectsUnknown),
            }
            let home = ItemLocation::Character(owner.clone());
            self.active.push(id.clone());
            let pulled = self.space(&home, &item);
            self.active.pop();
            pulled?;
            self.steps.push(BuildApplyStep {
                phase: self.phase,
                action: BuildAction::PullFromPostmaster {
                    item: id.clone(),
                    character: owner,
                },
                needs: (start..self.steps.len()).collect(),
            });
            self.relocate(id, &home);
            item = self.item(id);
            if &item.location == target {
                return Ok(());
            }
        }
        if item.notransfer(self.definition(&item)?) {
            return Err(ApplySkipReason::NotTransferable);
        }
        if !matches!(
            item.location,
            ItemLocation::Character(_) | ItemLocation::Vault
        ) {
            return Err(ApplySkipReason::UnsupportedLocation);
        }
        self.active.push(id.clone());
        let result = (|| {
            self.dequip(id)?;
            if item.location != ItemLocation::Vault && *target != ItemLocation::Vault {
                self.leg(id, &ItemLocation::Vault, start)?;
            }
            self.leg(id, target, start)
        })();
        self.active.pop();
        result
    }
    fn relocate(&mut self, id: &ItemInstanceId, to: &ItemLocation) {
        let changed = self
            .items
            .iter_mut()
            .find(|i| &i.instance_id == id)
            .expect("planner-owned identity");
        changed.location = to.clone();
        changed.equipped = false;
    }
    fn leg(
        &mut self,
        id: &ItemInstanceId,
        target: &ItemLocation,
        start: usize,
    ) -> Result<(), ApplySkipReason> {
        let item = self.item(id);
        self.space(target, &item)?;
        let mut needs: Vec<_> = (start..self.steps.len()).collect();
        needs.extend(self.pinned.iter().filter(|(i, _)| i == id).map(|(_, n)| *n));
        needs.sort_unstable();
        needs.dedup();
        self.steps.push(BuildApplyStep {
            phase: self.phase,
            action: BuildAction::Move {
                item: id.clone(),
                from: item.location,
                to: target.clone(),
            },
            needs,
        });
        self.relocate(id, target);
        Ok(())
    }
    fn attempt<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ApplySkipReason>,
    ) -> Result<T, ApplySkipReason> {
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
const WEAPON_BUCKETS: [i64; 3] = [1498876634, 2465295065, 953998645];
const GHOST_BUCKET: i64 = 4023194814;
const EMBLEM_BUCKET: i64 = 4274335291;
const SHIP_BUCKET: i64 = 284967655;
const SPARROW_BUCKET: i64 = 2025709351;
/// DIM's bucket order: weapons, armor, then the General category's Subclass,
/// Artifacts, Ghost, Emblems, Ships and Vehicle (`d2-bucket-categories.ts:5-30`,
/// `loadout-apply.ts:118-129`).
const BUCKETS: [i64; 14] = [
    1498876634,
    2465295065,
    953998645,
    3448274439,
    3551918588,
    14239492,
    20886954,
    1585787867,
    SUBCLASS_BUCKET_HASH.value(),
    ARTIFACT_BUCKET_HASH.value(),
    GHOST_BUCKET,
    EMBLEM_BUCKET,
    SHIP_BUCKET,
    SPARROW_BUCKET,
];
/// Equippable buckets outside the audited capacity policy (DIM treats them as
/// ordinary items; Ghostlink only equips them in place, never moves them).
pub fn is_equip_only_bucket(bucket: BucketHash) -> bool {
    BUCKETS[8..].contains(&bucket.value())
}
impl Planner<'_> {
    /// Equip with DIM's one-exotic handling: dequip conflicting labels first.
    /// Deviation from DIM's bucket order: Bungie EquipItems applies the list in
    /// order, so labeled (exotic) items go last, after the items that replace an
    /// equipped exotic; otherwise they fail with 1641 ExoticLimit (BE-010c live).
    fn equip_resolving(
        &mut self,
        mut ids: Vec<ItemInstanceId>,
        target: &CharacterId,
    ) -> Result<(), ApplySkipReason> {
        let destination = ItemLocation::Character(target.clone());
        for id in ids.clone() {
            let item = self.item(&id);
            let bucket = self.bucket(&item)?;
            let conflicts: Vec<_> = self
                .items
                .iter()
                .filter(|other| {
                    other.equipped
                        && other.location == destination
                        && self.label(&item).is_some()
                        && self.label(other) == self.label(&item)
                        && self.bucket(other) != Ok(bucket)
                        && !ids
                            .iter()
                            .any(|id| self.bucket(&self.item(id)) == self.bucket(other))
                })
                .cloned()
                .collect();
            for other in conflicts {
                let replacement = self.replacement(&other, target, &ids)?;
                self.move_item(&replacement, &destination)?;
                ids.push(replacement);
            }
        }
        ids.sort_by_key(|id| self.label(&self.item(id)).is_some());
        self.equip(ids, target)
    }
    /// DIM clearSpaceAfterLoadout/clearItemsOffCharacter (loadout-apply.ts:822-955).
    fn clear_space(
        &mut self,
        target: &CharacterId,
        build_items: &[ItemInstanceId],
        weapons: bool,
        armor: bool,
    ) -> Vec<(ItemInstanceId, ApplySkipReason)> {
        let home = ItemLocation::Character(target.clone());
        let mut remove = vec![];
        let mut reserve = vec![];
        let mut seen = vec![];
        for id in build_items {
            let Ok(bucket) = self.bucket(&self.item(id)) else {
                continue;
            };
            if is_equip_only_bucket(bucket) {
                continue;
            }
            let weapon = WEAPON_BUCKETS.contains(&bucket.value());
            if seen.contains(&bucket) || !(if weapon { weapons } else { armor }) {
                continue;
            }
            seen.push(bucket);
            let mut kept = 0;
            for existing in self
                .items
                .iter()
                .filter(|i| i.location == home && !i.equipped && self.bucket(i) == Ok(bucket))
            {
                let pinned = self
                    .definition(existing)
                    .is_ok_and(|d| existing.notransfer(d));
                if pinned || build_items.contains(&existing.instance_id) {
                    kept += 1;
                } else {
                    remove.push(existing.instance_id.clone());
                }
            }
            let capacity = self.policy.capacity(bucket).unwrap_or(0);
            reserve.push((bucket, capacity.saturating_sub(kept)));
        }
        self.reserve = Some((target.clone(), reserve));
        self.excluded.extend(remove.iter().cloned());
        let mut skipped = vec![];
        for id in remove {
            let bucket = self.bucket(&self.item(&id));
            let result = self.attempt(|p| {
                let bucket = bucket?;
                let vault_free = p.free(&ItemLocation::Vault, bucket)?;
                if vault_free <= 1 {
                    // Down to one Vault slot: prefer another character with space.
                    let other = p.characters.iter().map(|c| c.id.clone()).find(|c| {
                        c != target
                            && p.free(&ItemLocation::Character(c.clone()), bucket)
                                .is_ok_and(|n| n > 0)
                    });
                    if let Some(other) = other {
                        return p.move_item(&id, &ItemLocation::Character(other));
                    }
                    if vault_free == 0 {
                        return Err(ApplySkipReason::NoSpace);
                    }
                }
                p.move_item(&id, &ItemLocation::Vault)
            });
            if let Err(reason) = result {
                skipped.push((id, reason));
            }
        }
        skipped
    }
}
/// Whether an item move, pull or equip step handled `item` (a plug insertion needs it).
fn step_touches(action: &BuildAction, item: &ItemInstanceId) -> bool {
    match action {
        BuildAction::Move { item: moved, .. }
        | BuildAction::PullFromPostmaster { item: moved, .. } => moved == item,
        BuildAction::Equip { items, .. } => items.contains(item),
        BuildAction::InsertPlug { .. } => false,
    }
}
/// Vault evidence is explicit, never inferred from character capacities.
/// The current production projection cannot supply it; previews report that gap.
pub fn plan_build_items(
    target: &CharacterId,
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    characters: &[Character],
    policy: &EquipmentCapacityPolicy,
    context: &ApplyContext,
) -> BuildApplyPlan {
    let mut p = Planner {
        items: inventory.to_vec(),
        defs,
        characters,
        policy,
        vault: context.vault,
        loadouts: context.loadouts,
        reserve: None,
        excluded: selected
            .iter()
            .filter_map(|s| s.inventory.as_ref().map(|i| i.instance_id.clone()))
            .collect(),
        active: vec![],
        steps: vec![],
        phase: ApplyPhase::Dequip,
        hard_from: None,
        pinned: vec![],
    };
    let has_overrides = selected.iter().any(|s| !s.spec.socket_overrides.is_empty());
    let mut plan = BuildApplyPlan {
        target: target.clone(),
        steps: vec![],
        skipped: vec![],
        already_there: vec![],
        clear_skipped: vec![],
        socket_skipped: vec![],
        socket_already_there: vec![],
        mod_skipped: vec![],
        mods_in_place: 0,
        not_applied: [(
            has_overrides && context.catalogs.is_none(),
            "socket_overrides_evidence_unavailable",
        )]
        .into_iter()
        .filter_map(|(pending, name)| pending.then_some(name))
        .collect(),
        unknown: [
            (context.vault.is_none(), "vault_capacity"),
            (context.loadouts.is_none(), "in_game_loadout_membership"),
            (
                has_overrides && context.catalogs.is_none(),
                "socket_catalogs",
            ),
        ]
        .into_iter()
        .filter_map(|(missing, name)| missing.then_some(name))
        .collect(),
    };
    let mut order: Vec<_> = selected.iter().enumerate().collect();
    order.sort_by_key(|(_, s)| {
        s.inventory
            .as_ref()
            .and_then(|i| p.bucket(i).ok())
            .and_then(|b| BUCKETS.iter().position(|h| *h == b.value()))
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
            let definition = p.definition(item)?;
            if definition.max_stack_size.is_some_and(|n| n > 1)
                || definition.is_instance_item == Some(false)
            {
                return Err(ApplySkipReason::StackUnsupported);
            }
            let pulled_home = item.location == ItemLocation::Postmaster(target.clone());
            if item.location != destination && !pulled_home && item.notransfer(definition) {
                return Err(ApplySkipReason::NotTransferable);
            }
            if item.location != destination
                && !pulled_home
                && matches!(
                    item.location,
                    ItemLocation::Character(_) | ItemLocation::Postmaster(_)
                )
                && context.vault.is_none()
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
    // DIM counts equips before moving (loadout-apply.ts:424-433): one equip is
    // handled inline with its move, several use one bulk equip afterwards.
    let bulk = pending
        .iter()
        .filter(|(_, id, equip)| {
            let item = p.item(id);
            *equip && !(item.location == destination && item.equipped)
        })
        .count()
        > 1;
    // Equipped items on other characters: a lone one dequips inline when moved,
    // several are dequipped together per owner first (loadout-apply.ts:437-513).
    let mut owners: Vec<(CharacterId, Vec<(usize, ItemInstanceId)>)> = vec![];
    for (index, id, _) in &pending {
        let item = p.item(id);
        if let (true, ItemLocation::Character(owner)) = (item.equipped, &item.location)
            && owner != target
        {
            match owners.iter_mut().find(|(o, _)| o == owner) {
                Some((_, group)) => group.push((*index, id.clone())),
                None => owners.push((owner.clone(), vec![(*index, id.clone())])),
            }
        }
    }
    let mut dequip_failed = vec![];
    for (owner, group) in owners.iter().filter(|(_, g)| g.len() > 1) {
        let home = ItemLocation::Character(owner.clone());
        let group_start = p.steps.len();
        let mut replacements = vec![];
        for (index, id) in group {
            let result = p.attempt(|p| {
                let item = p.item(id);
                let replacement = p.replacement(&item, owner, &replacements)?;
                p.move_item(&replacement, &home)?;
                Ok(replacement)
            });
            match result {
                Ok(replacement) => replacements.push(replacement),
                Err(reason) => {
                    dequip_failed.push(*index);
                    plan.skipped.push(BuildApplySkip {
                        spec_index: *index,
                        reason,
                    });
                }
            }
        }
        if !replacements.is_empty() {
            p.hard_from = Some(group_start);
            let equipped = p.attempt(|p| p.equip(replacements, owner));
            p.hard_from = None;
            match equipped {
                Ok(()) => {
                    let equip = p.steps.len() - 1;
                    for (index, id) in group {
                        if !dequip_failed.contains(index) {
                            p.pinned.push((id.clone(), equip));
                        }
                    }
                }
                Err(reason) => {
                    let rest: Vec<_> = group
                        .iter()
                        .filter(|(i, _)| !dequip_failed.contains(i))
                        .collect();
                    for (index, _) in rest {
                        dequip_failed.push(*index);
                        plan.skipped.push(BuildApplySkip {
                            spec_index: *index,
                            reason,
                        });
                    }
                }
            }
        }
    }
    pending.retain(|(index, _, _)| !dequip_failed.contains(index));
    p.phase = ApplyPhase::Move;
    let mut equips = vec![];
    for (index, id, equip) in pending {
        let item = p.item(&id);
        if item.location == destination && (!equip || item.equipped) {
            plan.already_there.push(index);
            continue;
        }
        let inline = equip && !bulk;
        let result = p.attempt(|p| {
            p.move_item(&id, &destination)?;
            if inline {
                p.equip_resolving(vec![id.clone()], target)?;
            }
            Ok(())
        });
        match result {
            Err(reason) => plan.skipped.push(BuildApplySkip {
                spec_index: index,
                reason,
            }),
            Ok(()) if equip && bulk => equips.push((index, id)),
            Ok(()) => {}
        }
    }
    p.phase = ApplyPhase::Equip;
    if !equips.is_empty() {
        let ids: Vec<_> = equips.iter().map(|(_, id)| id.clone()).collect();
        if let Err(reason) = p.attempt(|p| p.equip_resolving(ids, target)) {
            for (index, _) in equips {
                plan.skipped.push(BuildApplySkip {
                    spec_index: index,
                    reason,
                });
            }
        }
    }
    // DIM applies socket overrides after the equips, then mods, before clear-space
    // (`loadout-apply.ts:629-672`). Items are judged where the plan leaves them.
    let after: Vec<_> = selected
        .iter()
        .map(|s| ResolvedBuildItem {
            inventory: s.inventory.as_ref().map(|i| {
                p.items
                    .iter()
                    .find(|moved| moved.instance_id == i.instance_id)
                    .unwrap_or(i)
                    .clone()
            }),
            ..s.clone()
        })
        .collect();
    if let (true, Some(catalogs)) = (has_overrides, context.catalogs) {
        p.phase = ApplyPhase::Sockets;
        let sockets =
            crate::build_sockets::plan_socket_overrides(target, &after, defs, characters, catalogs);
        for insert in sockets.inserts {
            p.push_insert(
                ApplyPhase::Sockets,
                target,
                insert.item,
                insert.socket_index,
                insert.plug,
            );
        }
        plan.socket_skipped = sockets.skipped;
        plan.socket_already_there = sockets.already_there;
    }
    if let Some(input) = &context.armor_mods {
        p.phase = ApplyPhase::Mods;
        let armor = crate::build_mod_apply::armor_set(target, &after, &p.items, defs);
        let mods = crate::build_mod_apply::plan_armor_mods(input, defs, target, &armor);
        for insert in mods.inserts {
            p.push_insert(
                ApplyPhase::Mods,
                target,
                insert.item,
                insert.socket_index,
                insert.plug,
            );
        }
        plan.mod_skipped = mods.skipped;
        plan.mods_in_place = mods.in_place;
        plan.not_applied.extend(mods.not_applied);
        // Fashion follows every mod insertion, as in DIM (`loadout-apply.ts:1380-1389`).
        let fashion = crate::build_fashion_apply::plan_fashion(input, defs, target, &armor);
        for insert in fashion.inserts {
            p.push_insert(
                ApplyPhase::Mods,
                target,
                insert.item,
                insert.socket_index,
                insert.plug,
            );
        }
        plan.mod_skipped.extend(fashion.skipped);
        plan.mods_in_place += fashion.in_place;
    }
    if context.clear_weapons || context.clear_armor {
        p.phase = ApplyPhase::ClearSpace;
        let build_items: Vec<_> = selected
            .iter()
            .filter_map(|s| s.inventory.as_ref().map(|i| i.instance_id.clone()))
            .collect();
        plan.clear_skipped = p.clear_space(
            target,
            &build_items,
            context.clear_weapons,
            context.clear_armor,
        );
    }
    plan.skipped.sort_by_key(|s| s.spec_index);
    plan.steps = p.steps;
    plan
}
