//! Pure DIM fashion application: the `modsByBucket` patch of `applyLoadoutMods`
//! (`loadout-apply.ts:308-338,1365-1405`), `equipModsToItem` (`:1489-1507`) and
//! `insertPlug` with `checkIrreversiblePlugging` (`advanced-write-actions.ts:105-173`),
//! commit 29c122e. Only free insertions are planned, as in the socket phase.
use crate::{
    build_analysis::ResolvedBuildItem,
    build_mod_apply::*,
    build_sockets::{Socket, can_insert_free, sockets},
    mod_assignment::ARMOR_BUCKETS,
    *,
};
use std::collections::BTreeMap;

/// `SocketCategoryHashes.ArmorCosmetics` (`data/d2/generated-enums.ts:1599`).
pub const ARMOR_COSMETICS_CATEGORY: u32 = 1926152773;
/// `ItemCategoryHashes.Shaders` (`data/d2/generated-enums.ts:1551`).
const SHADERS_CATEGORY: u32 = 41;
/// `DEFAULT_SHADER` and `DEFAULT_ORNAMENTS` (`search/d2-known-values.ts:42-53`).
const DEFAULT_SHADER: u32 = 4248210736;
pub const DEFAULT_ORNAMENTS: [u32; 4] = [2931483505, 1959648454, 702981643, 3854296178];
/// `DestinyItemType.Armor`, `DestinyItemSubType.Shader` and `.Ornament` (bungie-api-ts).
const ITEM_TYPE_ARMOR: u8 = 2;
const SUB_TYPE_SHADER: u8 = 20;
const SUB_TYPE_ORNAMENT: u8 = 21;

/// DIM `isFashionPlug` (`loadout-utils.ts:857-864`).
fn is_fashion_plug(definition: &ItemDefinition) -> bool {
    matches!(
        definition.armor.item_sub_type_code,
        Some(SUB_TYPE_SHADER | SUB_TYPE_ORNAMENT)
    ) || definition.armor.item_type_code == Some(ITEM_TYPE_ARMOR)
}

/// `unlockedExoticOrnamentsSelector` (`inventory/selectors.ts:372-389`): unlocks are
/// account-wide and only visible as `canInsert` on the cosmetic sockets of exotic armor.
pub fn exotic_ornament_unlocks(items: &[ItemInstance], defs: &[ItemDefinition]) -> Vec<u32> {
    let mut unlocked = std::collections::BTreeSet::new();
    for item in items {
        let Some(definition) = defs.iter().find(|d| d.hash == item.item_hash) else {
            continue;
        };
        if definition.tier.as_deref() != Some("Exotic")
            || !definition
                .bucket_hash
                .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
        {
            continue;
        }
        for socket in &item.sockets {
            let cosmetic = definition.armor.socket_categories.iter().any(|c| {
                c.socket_category_hash == ARMOR_COSMETICS_CATEGORY
                    && c.socket_indexes.contains(&u32::from(socket.socket_index))
            });
            for plug in socket.reusable_plugs.iter().flatten() {
                let hash = plug.plug_hash.value() as u32;
                if cosmetic && plug.can_insert && !DEFAULT_ORNAMENTS.contains(&hash) {
                    unlocked.insert(hash);
                }
            }
        }
    }
    unlocked.into_iter().collect()
}

/// An armor piece with the sockets and plugs as the earlier insertions leave them.
struct Piece<'a> {
    item: &'a ItemInstance,
    sockets: Vec<Socket<'a>>,
    current: BTreeMap<u8, Option<u32>>,
}
struct Action {
    piece: usize,
    socket: u8,
    plug: u32,
}

/// DIM `plugFitsIntoSocket` (`utils/socket-utils.ts:228-237`).
fn fits(socket: &Socket, item: &ItemInstance, plug: u32) -> bool {
    socket.empty == Some(plug)
        || socket.set_plugs.contains(&plug)
        || item
            .sockets
            .iter()
            .find(|s| s.socket_index == socket.index)
            .and_then(|s| s.reusable_plugs.as_ref())
            .is_some_and(|plugs| plugs.iter().any(|p| p.plug_hash.value() as u32 == plug))
}

/// Plans the shader and ornament insertions of the build's `modsByBucket` for `armor`
/// (see `build_mod_apply::armor_set`); items must be where the rest of the plan leaves
/// them. Plugs that are not fashion are ordered with the mods, fashion last.
pub fn plan_fashion(
    input: &ModInput,
    defs: &[ItemDefinition],
    target: &CharacterId,
    armor: &[ResolvedBuildItem],
) -> ModPlan {
    let mut plan = ModPlan::default();
    let Some(by_bucket) = input
        .build
        .parameters
        .as_ref()
        .and_then(|p| p.mods_by_bucket.as_ref())
        .filter(|buckets| !buckets.is_empty())
    else {
        return plan;
    };
    let catalogs = input.fashion_catalogs;
    let definition_of = |hash: u32| defs.iter().find(|d| d.hash.value() == i64::from(hash));
    let skip = |plan: &mut ModPlan, plug: u32, reason: &'static str| {
        plan.skipped.push(ModSkip {
            plug: ItemHash::new(i64::from(plug)),
            reason,
        });
    };
    // DIM `checkMod` (`loadout-apply.ts:308-320`): known, and unlocked or a default.
    let unlocked = |hash: u32| {
        definition_of(hash).is_some()
            && (input.unlocked.contains(&hash)
                || input.ornament_unlocks.contains(&hash)
                || hash == DEFAULT_SHADER
                || DEFAULT_ORNAMENTS.contains(&hash))
    };
    let mut pieces: Vec<Piece> = vec![];
    let mut mods = vec![];
    let mut fashion = vec![];
    let mut covered = vec![];
    for resolved in armor {
        let Some(item) = resolved.inventory.as_ref() else {
            continue;
        };
        let Some(definition) = definition_of(item.item_hash.value() as u32) else {
            continue;
        };
        let Some(bucket) = definition.bucket_hash else {
            continue;
        };
        covered.push(bucket);
        let Some(requested) = by_bucket.get(&bucket) else {
            continue;
        };
        let hashes: Vec<u32> = requested.iter().map(|h| h.value() as u32).collect();
        let hashes: Vec<u32> = hashes
            .into_iter()
            .filter(|h| {
                let known = unlocked(*h);
                if !known {
                    skip(&mut plan, *h, "fashion_not_unlocked");
                }
                known
            })
            .collect();
        let refusal = if item.location != ItemLocation::Character(target.clone()) {
            Some("not_on_target_character")
        } else if item.sockets.is_empty() {
            Some("socket_state_unavailable")
        } else {
            None
        };
        if let Some(reason) = refusal {
            hashes.iter().for_each(|h| skip(&mut plan, *h, reason));
            continue;
        }
        let states = sockets(item, definition, catalogs);
        let piece = pieces.len();
        for hash in hashes {
            let plug = definition_of(hash).expect("checked by `unlocked`");
            // DIM assigns to the first socket the plug fits (`loadout-apply.ts:1365-1380`).
            let socket = states
                .iter()
                .find(|s| fits(s, item, hash))
                .filter(|_| plug.armor.plug_category_hash.is_some());
            match socket {
                Some(socket) => {
                    let action = Action {
                        piece,
                        socket: socket.index,
                        plug: hash,
                    };
                    if is_fashion_plug(plug) {
                        fashion.push(action);
                    } else {
                        mods.push(action);
                    }
                }
                None => skip(&mut plan, hash, "fashion_unassigned"),
            }
        }
        pieces.push(Piece {
            item,
            current: states.iter().map(|s| (s.index, s.plugged)).collect(),
            sockets: states,
        });
    }
    for (bucket, hashes) in by_bucket {
        if !covered.contains(bucket) {
            for hash in hashes
                .iter()
                .map(|h| h.value() as u32)
                .filter(|h| unlocked(*h))
            {
                skip(&mut plan, hash, "fashion_no_armor");
            }
        }
    }
    // `equipModsToItem` runs all mods before all fashion (`loadout-apply.ts:1388-1405`).
    for action in mods.into_iter().chain(fashion) {
        let piece = &mut pieces[action.piece];
        let socket = piece
            .sockets
            .iter()
            .find(|s| s.index == action.socket)
            .expect("assigned socket");
        let mut plug = action.plug;
        // A transmog ornament cannot go onto the item it was made from; DIM plugs the
        // socket's default instead (`loadout-apply.ts:1489-1500`).
        if plug as i64 == piece.item.item_hash.value()
            && let Some(empty) = socket.empty
        {
            plug = empty;
        }
        let current = piece.current[&action.socket];
        if current == Some(plug) {
            plan.in_place += 1;
            continue;
        }
        if let Err(reason) = can_insert_free(defs, catalogs, socket.entry, current, plug) {
            skip(&mut plan, action.plug, reason.as_str());
            continue;
        }
        if protected(input, piece.item, socket, current, definition_of) {
            skip(&mut plan, action.plug, "irreversible_shader");
            continue;
        }
        piece.current.insert(action.socket, Some(plug));
        plan.inserts.push(ModInsert {
            item: piece.item.instance_id.clone(),
            socket_index: action.socket,
            plug: ItemHash::new(i64::from(plug)),
        });
    }
    plan
}

/// DIM `checkIrreversiblePlugging` (`advanced-write-actions.ts:105-135`): a plugged
/// shader (other than the defaults) that is neither unlocked in the socket's plug set
/// nor enabled among the item's reusable plugs cannot be plugged back.
fn protected<'a>(
    input: &ModInput,
    item: &ItemInstance,
    socket: &Socket,
    current: Option<u32>,
    definition_of: impl Fn(u32) -> Option<&'a ItemDefinition>,
) -> bool {
    let Some(plugged) = current else {
        return false;
    };
    let is_shader = definition_of(plugged)
        .is_some_and(|d| d.armor.item_category_hashes.contains(&SHADERS_CATEGORY));
    if !is_shader || plugged == DEFAULT_SHADER || DEFAULT_ORNAMENTS.contains(&plugged) {
        return false;
    }
    let profile_unlocked = socket.entry.reusable_plug_set_hash.is_some_and(|h| h != 0)
        && input.unlocked.contains(&plugged);
    let item_unlocked = item
        .sockets
        .iter()
        .find(|s| s.socket_index == socket.index)
        .and_then(|s| s.reusable_plugs.as_ref())
        .is_some_and(|plugs| {
            plugs
                .iter()
                .any(|p| p.enabled && p.plug_hash.value() as u32 == plugged)
        });
    !profile_unlocked && !item_unlocked
}
