//! Equipped (hypothetical build) set activation, DIM SetBonus.tsx:23-64.
use crate::{armor_catalog::*, build_analysis::ResolvedBuildItem, *};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquippedSetBonuses {
    pub evaluation: &'static str,
    pub reasons: Vec<&'static str>,
    pub sets: Vec<EquippedSetBonus>,
}
impl Default for EquippedSetBonuses {
    fn default() -> Self {
        Self {
            evaluation: "not_evaluated",
            reasons: vec!["set_activation_not_assembled"],
            sets: vec![],
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquippedSetBonus {
    pub hash: u32,
    pub name: Option<String>,
    /// Observed contributing pieces; a lower bound when evaluation is partial.
    pub equipped_count: usize,
    /// None means the set catalog is unavailable, not an empty perk list.
    pub perks: Option<Vec<EquippedSetPerk>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquippedSetPerk {
    pub perk: SetPerk,
    pub active: Option<bool>,
}
fn reason(reasons: &mut Vec<&'static str>, value: &'static str) {
    if !reasons.contains(&value) {
        reasons.push(value);
    }
}
/// Native set identity takes precedence over event selector sockets. Selectors
/// require visibility and enabled, unlike ordinary native set contributions.
/// DIM socket-utils.ts:782-809; known-values.ts:51-71 (commit 29c122e).
fn active_set(
    item: &ItemInstance,
    definition: &ItemDefinition,
    definitions: &[ItemDefinition],
) -> Result<Option<u32>, &'static str> {
    if let Some(hash) = definition.armor.equipable_item_set_hash.filter(|h| *h != 0) {
        return Ok(Some(hash));
    }
    if item.sockets.is_empty() {
        return Err("set_selector_sockets_unavailable");
    }
    for socket in item.sockets.iter().filter(|s| s.visible) {
        let Some(hash) = socket.plug_hash else {
            continue;
        };
        let Some(plug) = definitions.iter().find(|d| d.hash == hash) else {
            // Cannot rule out an earlier selector in DIM's first-socket scan.
            return Err("set_selector_plug_definition_unavailable");
        };
        if plug.armor.plug_category_hash != Some(1313063513) {
            continue;
        }
        if !socket.enabled {
            return Ok(None);
        }
        return selector_set(hash.value() as u32)
            .map(Some)
            .ok_or("set_selector_mapping_unavailable");
    }
    Ok(None)
}
fn selector_set(hash: u32) -> Option<u32> {
    Some(match hash {
        139044974 => 2132906400,
        139044987 => 2391762223,
        721111598 => 239346083,
        721111611 => 3252452908,
        1012508294 => 2258577662,
        1012508307 => 3734029045,
        1220635053 => 3259216565,
        1220635064 => 1083114430,
        1404854454 => 1223381128,
        1530138662 => 2450108908,
        1841728090 => 4222859846,
        2824493179 => 1007956300,
        2872740129 => 2151917545,
        3573256294 => 2947197258,
        3573256307 => 2751989785,
        3782433407 => 3737690559,
        3834187337 => 305966751,
        3874641219 => 2839368623,
        4119627352 => 894715166,
        _ => return None,
    })
}

pub fn equipped_set_bonuses(
    items: &[ResolvedBuildItem],
    definitions: &[ItemDefinition],
    catalogs: &[EquipableSetDefinition],
) -> EquippedSetBonuses {
    let mut counts = BTreeMap::<u32, usize>::new();
    let mut reasons = Vec::new();
    for resolved in items.iter().filter(|i| i.should_equip) {
        let hash = resolved
            .inventory
            .as_ref()
            .map_or(resolved.spec.hash, |i| i.item_hash);
        let Some(definition) = definitions.iter().find(|d| d.hash == hash) else {
            reason(&mut reasons, "equipped_item_definition_unavailable");
            continue;
        };
        let Some(bucket) = definition.bucket_hash else {
            reason(&mut reasons, "equipped_item_bucket_unavailable");
            continue;
        };
        if !matches!(
            bucket.value(),
            3448274439 | 3551918588 | 14239492 | 20886954 | 1585787867
        ) {
            continue;
        }
        let Some(item) = &resolved.inventory else {
            reason(&mut reasons, "equipped_armor_unresolved");
            continue;
        };
        match active_set(item, definition, definitions) {
            Ok(Some(hash)) => *counts.entry(hash).or_default() += 1,
            Ok(None) => {}
            Err(value) => reason(&mut reasons, value),
        }
    }
    // Unknown contributors can increase any count, but cannot remove known
    // contributions. Missing catalogs only obscure their own perk thresholds.
    let unknown_contributors = !reasons.is_empty();
    let sets = counts
        .into_iter()
        .map(|(hash, equipped_count)| {
            let catalog = catalogs.iter().find(|s| s.hash == hash);
            if catalog.is_none() {
                reason(&mut reasons, "equipable_set_definition_unavailable");
            }
            EquippedSetBonus {
                hash,
                name: catalog.map(|s| s.name.clone()),
                equipped_count,
                perks: catalog.map(|s| {
                    s.set_perks
                        .iter()
                        .map(|perk| EquippedSetPerk {
                            active: if equipped_count as i64 >= i64::from(perk.required_set_count) {
                                Some(true)
                            } else if unknown_contributors {
                                None
                            } else {
                                Some(false)
                            },
                            perk: perk.clone(),
                        })
                        .collect()
                }),
            }
        })
        .collect();
    EquippedSetBonuses {
        evaluation: if reasons.is_empty() {
            "evaluated"
        } else {
            "partial"
        },
        reasons,
        sets,
    }
}
