//! Pure DIM armor-mod application: `applyLoadoutMods` (`loadout-apply.ts:1260-1405`),
//! `pickPlugPositions` and `createPluggingStrategy` (`mod-assignment-utils.ts:689-1000`),
//! commit 29c122e. Mods are fitted with the as-is energy rules (`inGameArmorEnergyRules`)
//! by the existing assignment; only free insertions are planned, as in the socket phase.
use crate::{
    armor_catalog::*,
    build::BuildDefinition,
    build_analysis::ResolvedBuildItem,
    build_sockets::{can_insert_free, sockets},
    mod_assignment::{
        ARMOR_BUCKETS, apply_mod_assignments, assignment_sockets, exclusion_group,
        resolve_mod_cost_variant,
    },
    *,
};

/// DIM `getArmorSocketsAndMods(.., assumeArtificeUnlocked)` (`mod-assignment-utils.ts:586-593`).
const ARTIFICE_EMPTY_PLUG: u32 = 4173924323;
const MASTERWORK_SOCKET_TYPE: u32 = 1843767421;

/// Everything the mod phase reads; nothing here is fetched or mutated.
#[derive(Clone, Copy, Debug)]
pub struct ModInput<'a> {
    pub build: &'a BuildDefinition,
    /// Mods unlocked on the target character (profile plus character plug sets).
    pub unlocked: &'a [u32],
    /// Socket types, plug sets and material requirement sets of the armor.
    pub catalogs: &'a [ArmorCatalogRecord],
    /// Socket types and plug sets of the armor cosmetic sockets, and the material
    /// requirement sets of the fashion plugs (see `build_fashion_apply`).
    pub fashion_catalogs: &'a [ArmorCatalogRecord],
    /// Ornaments unlocked without an enabled plug-set entry: exotic ornaments owned
    /// on any exotic armor instance, and `canInsert` ornament entries.
    pub ornament_unlocks: &'a [u32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModInsert {
    pub item: ItemInstanceId,
    pub socket_index: u8,
    pub plug: ItemHash,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModSkip {
    pub plug: ItemHash,
    pub reason: &'static str,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModPlan {
    pub inserts: Vec<ModInsert>,
    pub skipped: Vec<ModSkip>,
    /// Requested mods already in their socket.
    pub in_place: usize,
    pub not_applied: Vec<&'static str>,
}

/// DIM mods go to the build's armor marked "equip", else the equipped piece of each
/// slot, in `ArmorBucketHashes` order (`loadout-apply.ts:1282-1296`).
pub fn armor_set(
    target: &CharacterId,
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
) -> Vec<ResolvedBuildItem> {
    let bucket = |item: &ItemInstance| {
        defs.iter()
            .find(|d| d.hash == item.item_hash)
            .and_then(|d| d.bucket_hash)
            .map(BucketHash::value)
    };
    ARMOR_BUCKETS
        .into_iter()
        .filter_map(|b| {
            selected
                .iter()
                .find(|r| r.should_equip && r.inventory.as_ref().and_then(bucket) == Some(b))
                .cloned()
                .or_else(|| {
                    inventory
                        .iter()
                        .find(|i| {
                            i.equipped
                                && i.location == ItemLocation::Character(target.clone())
                                && bucket(i) == Some(b)
                        })
                        .map(|item| ResolvedBuildItem {
                            spec: crate::build::BuildItem {
                                instance_id: Some(item.instance_id.clone()),
                                hash: item.item_hash,
                                amount: None,
                                socket_overrides: vec![],
                                crafted_date: None,
                            },
                            should_equip: true,
                            inventory: Some(item.clone()),
                            class_compatible: Some(true),
                            resolution_reason: None,
                        })
                })
        })
        .collect()
}

/// An armor mod socket as `pickPlugPositions` sees it.
struct Position<'a> {
    index: u8,
    plugged: u32,
    empty: Option<u32>,
    set_size: Option<usize>,
    fits: Vec<u32>,
    entry: &'a DefinitionSocketEntry,
}
/// DIM `Assignment`/`PluggingAction` (`mod-assignment-utils.ts:689-1000`).
#[derive(Clone)]
struct Action {
    socket: u8,
    plug: u32,
    requested: bool,
    required: bool,
    spend: i32,
    added: Option<&'static str>,
    released: Option<&'static str>,
}

fn cost(defs: &[ItemDefinition], hash: u32) -> i32 {
    defs.iter()
        .find(|d| d.hash.value() == i64::from(hash))
        .and_then(|d| d.armor.energy_cost.as_ref())
        .map_or(0, |c| c.energy_cost)
}

/// DIM `pickPlugPositions`: keep a mod where it already is, then replace a similar
/// (same exclusion group) plug, then take the first socket that fits. Remaining sockets
/// return to their empty plug, requested only with `clearMods`.
fn pick_plug_positions(
    defs: &[ItemDefinition],
    positions: &[Position],
    mods: &[u32],
    clear: bool,
) -> Result<Vec<(u8, u32, bool)>, &'static str> {
    let mut sockets: Vec<&Position> = positions.iter().collect();
    sockets.sort_by_key(|s| s.set_size.unwrap_or(999));
    let mut ordered: Vec<u32> = mods.to_vec();
    ordered.sort_by_key(|m| sockets.iter().filter(|s| s.fits.contains(m)).count());
    let mut picked = vec![];
    for m in ordered {
        let at = sockets
            .iter()
            .position(|s| s.plugged == m)
            .or_else(|| {
                sockets.iter().position(|s| {
                    s.fits.contains(&m) && exclusion_group(s.plugged) == exclusion_group(m)
                })
            })
            .or_else(|| sockets.iter().position(|s| s.fits.contains(&m)))
            .ok_or("no_socket_for_mod")?;
        picked.push((sockets[at].index, m, true));
        sockets.remove(at);
    }
    for s in sockets {
        if let Some(empty) = s
            .empty
            .filter(|e| defs.iter().any(|d| d.hash.value() == i64::from(*e)))
        {
            picked.push((s.index, empty, clear));
        }
    }
    Ok(picked)
}

/// DIM `createPluggingStrategy`: regains first, then spends, freeing energy or an
/// exclusion group with optional regains only when a spend needs it.
fn plugging_strategy(
    defs: &[ItemDefinition],
    positions: &[Position],
    capacity: i32,
    used: i32,
    assignments: Vec<(u8, u32, bool)>,
) -> Result<Vec<Action>, &'static str> {
    let (mut regains, mut spends, mut optional, mut ops) = (vec![], vec![], vec![], vec![]);
    for (socket, plug, requested) in assignments {
        let Some(at) = positions.iter().find(|p| p.index == socket) else {
            continue;
        };
        let existing = cost(defs, at.plugged);
        let planned = cost(defs, plug);
        let action = Action {
            socket,
            plug,
            requested,
            required: requested,
            spend: planned - existing,
            added: exclusion_group(plug),
            released: exclusion_group(at.plugged),
        };
        if at.plugged == plug {
            if action.required {
                ops.push(action);
            }
        } else if action.spend > 0 && action.released.is_none() {
            spends.push(action);
        } else if !action.required && at.empty == Some(plug) {
            optional.push(action);
        } else if action.spend <= 0 && action.added.is_none() {
            regains.push(action);
        } else {
            // A -> EMPTY -> ...other actions... -> B.
            regains.push(Action {
                socket,
                plug: at.empty.ok_or("empty_plug_unknown")?,
                requested: false,
                required: true,
                spend: -existing,
                added: None,
                released: action.released,
            });
            spends.push(Action {
                spend: planned,
                released: None,
                required: true,
                ..action
            });
        }
    }
    // Lower gains first, zero gains last (they would only pad the sum).
    optional.sort_by_key(|a: &Action| if a.spend < 0 { -a.spend } else { i32::MAX });
    let mut used = used;
    for regain in regains {
        used += regain.spend;
        ops.push(regain);
    }
    for spend in spends {
        while used + spend.spend > capacity
            || (spend.added.is_some() && optional.iter().any(|r| r.released == spend.added))
        {
            if optional.is_empty() {
                return Err("not_enough_energy");
            }
            let extra = spend.spend - (capacity - used);
            let which = spend
                .added
                .and_then(|_| optional.iter().position(|r| r.released == spend.added))
                .or_else(|| optional.iter().position(|r| -r.spend >= extra))
                .unwrap_or(0);
            let mut regain = optional.remove(which);
            regain.required = true;
            used += regain.spend;
            ops.push(regain);
        }
        used += spend.spend;
        ops.push(spend);
    }
    ops.extend(optional);
    ops.retain(|a| a.required);
    Ok(ops)
}

fn all_applied(armor: &[&ItemInstance], hashes: &[u32]) -> bool {
    let mut present: Vec<u32> = armor
        .iter()
        .flat_map(|i| i.sockets.iter())
        .filter_map(|s| s.plug_hash.map(|h| h.value() as u32))
        .collect();
    hashes
        .iter()
        .all(|h| match present.iter().position(|p| p == h) {
            Some(at) => {
                present.remove(at);
                true
            }
            None => false,
        })
}

fn skip_all(plan: &mut ModPlan, mods: &[u32], reason: &'static str) {
    plan.skipped.extend(mods.iter().map(|m| ModSkip {
        plug: ItemHash::new(i64::from(*m)),
        reason,
    }));
}

fn plan_item(
    plan: &mut ModPlan,
    input: &ModInput,
    defs: &[ItemDefinition],
    target: &CharacterId,
    item: &ItemInstance,
    mods: &[u32],
    clear: bool,
) {
    let Some(definition) = defs.iter().find(|d| d.hash == item.item_hash) else {
        return skip_all(plan, mods, "missing_definition");
    };
    if item.location != ItemLocation::Character(target.clone()) {
        return skip_all(plan, mods, "not_on_target_character");
    }
    let Some(energy) = &item.energy else {
        return skip_all(plan, mods, "item_energy_unavailable");
    };
    let assignable = match assignment_sockets(item, definition, defs, input.catalogs, false) {
        Ok(sockets) => sockets,
        Err(reason) => return skip_all(plan, mods, reason),
    };
    let states = sockets(item, definition, input.catalogs);
    let positions: Vec<Position> = assignable
        .into_iter()
        .filter_map(|a| {
            let state = states.iter().find(|s| s.index == a.index)?;
            let visible = item
                .sockets
                .iter()
                .any(|s| s.socket_index == a.index && s.visible);
            ((visible || state.empty == Some(ARTIFICE_EMPTY_PLUG)) // masterwork socket is redundant with the energy track
                && state.entry.socket_type_hash != MASTERWORK_SOCKET_TYPE)
                .then_some(Position {
                    index: a.index,
                    plugged: state.plugged?,
                    empty: state.empty,
                    set_size: a.plug_set_size,
                    fits: a.fits,
                    entry: state.entry,
                })
        })
        .collect();
    let plugged: Vec<(u8, u32)> = positions.iter().map(|p| (p.index, p.plugged)).collect();
    let actions = pick_plug_positions(defs, &positions, mods, clear).and_then(|assignments| {
        plugging_strategy(
            defs,
            &positions,
            energy.energy_capacity as i32,
            energy.energy_used as i32,
            assignments,
        )
    });
    let actions = match actions {
        Ok(actions) => actions,
        Err(reason) => return skip_all(plan, mods, reason),
    };
    // Each insertion changes the socket the next one sees (`canInsertForFree`).
    let mut current: std::collections::BTreeMap<u8, u32> = plugged.into_iter().collect();
    for action in actions {
        let position = positions
            .iter()
            .find(|p| p.index == action.socket)
            .expect("picked");
        if current.get(&action.socket) == Some(&action.plug) {
            plan.in_place += usize::from(action.requested);
            continue;
        }
        match can_insert_free(
            defs,
            input.catalogs,
            position.entry,
            current.get(&action.socket).copied(),
            action.plug,
        ) {
            Ok(()) => {
                current.insert(action.socket, action.plug);
                plan.inserts.push(ModInsert {
                    item: item.instance_id.clone(),
                    socket_index: action.socket,
                    plug: ItemHash::new(i64::from(action.plug)),
                });
            }
            Err(reason) if action.requested => skip_all(plan, &[action.plug], reason.as_str()),
            Err(_) => {}
        }
    }
}

/// Plans the free plug insertions that put the build's armor mods on `armor` (see
/// [`armor_set`]); items must be where the rest of the plan leaves them.
pub fn plan_armor_mods(
    input: &ModInput,
    defs: &[ItemDefinition],
    target: &CharacterId,
    armor: &[ResolvedBuildItem],
) -> ModPlan {
    let mut plan = ModPlan::default();
    let parameters = input.build.parameters.as_ref();
    let requested = parameters.and_then(|p| p.mods.as_deref()).unwrap_or(&[]);
    let clear = parameters.and_then(|p| p.clear_mods) == Some(true);
    if requested.is_empty() && !clear {
        return plan;
    }
    // DIM `checkMod`: a mod the character has not unlocked is dropped.
    let (mut kept, mut hashes) = (vec![], vec![]);
    for m in requested {
        let hash = resolve_mod_cost_variant(m.value() as u32, input.unlocked);
        if input.unlocked.contains(&hash) {
            kept.push(*m);
            hashes.push(hash);
        } else {
            skip_all(&mut plan, &[hash], "mod_not_unlocked");
        }
    }
    let items: Vec<&ItemInstance> = armor.iter().filter_map(|r| r.inventory.as_ref()).collect();
    if !clear && all_applied(&items, &hashes) {
        plan.in_place = hashes.len();
        return plan;
    }
    let mut build = input.build.clone();
    build.parameters.get_or_insert_with(Default::default).mods = Some(kept);
    let fit = apply_mod_assignments(&build, armor, defs, input.catalogs, input.unlocked);
    let (Some(assignments), Some(unassigned), Some(invalid)) = (
        fit.item_mod_assignments,
        fit.unassigned_mods,
        fit.invalid_mods,
    ) else {
        plan.not_applied
            .push(fit.reason.unwrap_or("armor_mod_assignment_unavailable"));
        return plan;
    };
    skip_all(&mut plan, &invalid, "invalid_for_armor");
    skip_all(&mut plan, &unassigned, "unassigned");
    for (id, mods) in assignments {
        if let Some(item) = items.iter().find(|i| i.instance_id == id) {
            plan_item(&mut plan, input, defs, target, item, &mods, clear);
        }
    }
    plan
}
