//! DIM armor-mod application: `applyLoadoutMods` (`loadout-apply.ts:1260-1405`),
//! `pickPlugPositions` and `createPluggingStrategy` (`mod-assignment-utils.ts:689-1000`),
//! commit 29c122e.
use destiny_core::{
    armor_catalog::*,
    build::*,
    build_analysis::ResolvedBuildItem,
    build_apply::*,
    build_mod_apply::*,
    domain::{ClassType, ReusablePlug},
    mod_assignment::ARMOR_BUCKETS,
    *,
};

const EMPTY: u32 = 235531041;
const HELMET_MOD_CATEGORY: u32 = 2912171003;

fn plug(hash: u32, cost: i32, requirement: Option<u32>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(i64::from(hash)),
        bucket_hash: None,
        name: "Mod".into(),
        tier: Some("Legendary".into()),
        item_type: Some("Armor Mod".into()),
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            plug_category_hash: Some(HELMET_MOD_CATEGORY),
            energy_cost: Some(PlugEnergyCost {
                energy_cost: cost,
                energy_type: 1,
                energy_type_hash: 1,
            }),
            insertion_material_requirement_hash: requirement,
            ..Default::default()
        },
        non_transferrable: None,
        is_instance_item: Some(false),
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: None,
        stack_unique_label: None,
    }
}
fn socket(index: u8, plugged: u32) -> IndexedSocket {
    IndexedSocket {
        plug_objectives: None,
        reusable_plugs: Some(vec![ReusablePlug {
            plug_hash: ItemHash::new(i64::from(plugged)),
            enabled: true,
            can_insert: true,
        }]),
        socket_index: index,
        plug_hash: Some(ItemHash::new(i64::from(plugged))),
        enabled: true,
        visible: true,
    }
}
fn resolved(item: &ItemInstance) -> ResolvedBuildItem {
    ResolvedBuildItem {
        spec: BuildItem {
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
    }
}
fn here() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("1"))
}

/// Five armor pieces (ids 1-5, bucket order) with two mod sockets each (0 and 1), both
/// holding the empty plug, energy 10 with none used. Plug set 800 offers every mod.
struct World {
    defs: Vec<ItemDefinition>,
    catalogs: Vec<ArmorCatalogRecord>,
    items: Vec<ItemInstance>,
}
fn world() -> World {
    let mut defs = vec![
        plug(EMPTY, 0, None),
        plug(2001, 3, None),
        plug(2002, 4, None),
        plug(2003, 6, None),
        plug(2004, 6, None),
        plug(2005, 1, Some(900)),
    ];
    let mut items = vec![];
    for (index, bucket) in ARMOR_BUCKETS.into_iter().enumerate() {
        let mut armor = plug(index as u32 + 11, 0, None);
        armor.bucket_hash = Some(BucketHash::new(bucket));
        armor.class_type = Some(ClassType::Hunter);
        armor.item_type = Some("Armor".into());
        armor.is_instance_item = Some(true);
        armor.armor = ArmorDefinition {
            socket_entries: vec![
                DefinitionSocketEntry {
                    socket_type_hash: 99,
                    reusable_plug_set_hash: Some(800),
                    plug_sources: Some(2),
                    ..Default::default()
                };
                2
            ],
            socket_categories: vec![DefinitionSocketCategory {
                socket_category_hash: 590099826,
                socket_indexes: vec![0, 1],
            }],
            ..Default::default()
        };
        items.push(ItemInstance {
            instance_id: ItemInstanceId::new((index + 1).to_string()),
            item_hash: armor.hash,
            location: here(),
            quantity: 1,
            equipped: true,
            locked: false,
            power: None,
            perks: vec![],
            plugs: vec![],
            stats: vec![],
            available_plugs: vec![],
            energy: Some(ItemEnergy {
                energy_capacity: 10,
                ..Default::default()
            }),
            gear_tier: None,
            transfer_status: Some(0),
            sockets: vec![socket(0, EMPTY), socket(1, EMPTY)],
        });
        defs.push(armor);
    }
    let catalogs = vec![
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash: 99,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        }),
        ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
            hash: 100,
            category_style: 3,
            name: "Mods".into(),
        }),
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash: 800,
            reusable_plug_items: [EMPTY, 2001, 2002, 2003, 2004, 2005]
                .map(|plug_item_hash| PlugSetItem {
                    plug_item_hash,
                    currently_can_roll: true,
                })
                .to_vec(),
        }),
        ArmorCatalogRecord::MaterialRequirementSet(MaterialRequirementSetDefinition {
            hash: 900,
            materials: vec![MaterialRequirement {
                item_hash: 1,
                count: 5,
            }],
        }),
    ];
    World {
        defs,
        catalogs,
        items,
    }
}
fn build(mods: &[u32], clear: bool) -> BuildDefinition {
    BuildDefinition {
        parameters: Some(BuildParameters {
            mods: Some(mods.iter().map(|m| ItemHash::new(i64::from(*m))).collect()),
            clear_mods: clear.then_some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}
impl World {
    fn plan(&self, build: &BuildDefinition, unlocked: &[u32], armor: &[ItemInstance]) -> ModPlan {
        let armor: Vec<_> = armor.iter().map(resolved).collect();
        let input = ModInput {
            build,
            unlocked,
            catalogs: &self.catalogs,
            fashion_catalogs: &[],
            ornament_unlocks: &[],
        };
        plan_armor_mods(&input, &self.defs, &CharacterId::new("1"), &armor)
    }
}
fn inserts(plan: &ModPlan) -> Vec<(&str, u8, i64)> {
    plan.inserts
        .iter()
        .map(|i| (i.item.as_str(), i.socket_index, i.plug.value()))
        .collect()
}

#[test]
fn a_bucket_mod_goes_into_the_first_fitting_socket_of_its_armor_piece() {
    let w = world();
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items);
    assert_eq!(inserts(&plan), [("1", 0, 2001)]);
    assert!(plan.skipped.is_empty() && plan.not_applied.is_empty());
    assert_eq!(plan.in_place, 0);
}

#[test]
fn mods_already_on_the_armor_need_no_request() {
    let mut w = world();
    w.items[0].sockets[1] = socket(1, 2001);
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items);
    assert!(plan.inserts.is_empty() && plan.skipped.is_empty());
    assert_eq!(plan.in_place, 1);
}

#[test]
fn locked_mods_are_dropped_before_fitting() {
    let w = world();
    let plan = w.plan(&build(&[2001], false), &[], &w.items);
    assert!(plan.inserts.is_empty());
    assert_eq!(
        plan.skipped,
        [ModSkip {
            plug: ItemHash::new(2001),
            reason: "mod_not_unlocked"
        }]
    );
}

#[test]
fn clear_mods_returns_other_sockets_to_empty_after_regaining_energy() {
    let mut w = world();
    w.items[0].sockets[1] = socket(1, 2002);
    w.items[0].energy.as_mut().unwrap().energy_used = 4;
    let wanted = build(&[2001], true);
    let plan = w.plan(&wanted, &[2001], &w.items);
    // The regain (socket 1 back to empty) is applied before the spend.
    assert_eq!(inserts(&plan), [("1", 1, EMPTY.into()), ("1", 0, 2001)]);
    // Without clearMods the optional reset is not needed and not sent.
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items);
    assert_eq!(inserts(&plan), [("1", 0, 2001)]);
}

#[test]
fn optional_regains_free_energy_only_when_a_spend_needs_it() {
    let mut w = world();
    w.items[0].sockets[1] = socket(1, 2004);
    w.items[0].energy.as_mut().unwrap().energy_used = 6;
    let plan = w.plan(&build(&[2003], false), &[2003], &w.items);
    assert_eq!(inserts(&plan), [("1", 1, EMPTY.into()), ("1", 0, 2003)]);
}

#[test]
fn only_free_insertions_are_planned() {
    let w = world();
    let plan = w.plan(&build(&[2005], false), &[2005], &w.items);
    assert!(plan.inserts.is_empty());
    assert_eq!(
        plan.skipped,
        [ModSkip {
            plug: ItemHash::new(2005),
            reason: "not_free"
        }]
    );
}

#[test]
fn armor_that_is_not_on_the_target_character_is_skipped_per_mod() {
    let mut w = world();
    w.items[0].location = ItemLocation::Vault;
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items);
    assert!(plan.inserts.is_empty());
    assert_eq!(
        plan.skipped,
        [ModSkip {
            plug: ItemHash::new(2001),
            reason: "not_on_target_character"
        }]
    );
}

#[test]
fn fewer_than_five_armor_pieces_still_receive_their_mods_like_dim() {
    // `fitMostMods` takes whatever armor `applyLoadoutMods` collected
    // (`loadout-apply.ts:1282-1296`, `mod-assignment-utils.ts:286-354`).
    let w = world();
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items[..1]);
    assert_eq!(inserts(&plan), [("1", 0, 2001)]);
    assert!(plan.skipped.is_empty() && plan.not_applied.is_empty());
    let plan = w.plan(&build(&[2001], false), &[2001], &w.items[..4]);
    assert_eq!(inserts(&plan), [("1", 0, 2001)]);
    assert!(plan.not_applied.is_empty());
}

#[test]
fn a_mod_with_no_armor_piece_to_hold_it_is_skipped_and_never_sent() {
    let w = world();
    // Other pieces could hold it but the helmet mod's bucket has no piece
    // (`mod-assignment-utils.ts:343-354`: unassigned); with no armor at all no piece
    // admits it (invalid for the set).
    for (armor, reason) in [
        (&w.items[1..], "unassigned"),
        (&w.items[..0], "invalid_for_armor"),
    ] {
        let plan = w.plan(&build(&[2001], false), &[2001], armor);
        assert!(plan.inserts.is_empty());
        assert!(plan.not_applied.is_empty());
        assert_eq!(
            plan.skipped,
            [ModSkip {
                plug: ItemHash::new(2001),
                reason
            }]
        );
    }
}

#[test]
fn the_armor_set_prefers_build_pieces_then_the_equipped_ones() {
    let w = world();
    let mut build_helmet = w.items[0].clone();
    build_helmet.instance_id = ItemInstanceId::new("9");
    build_helmet.equipped = false;
    let mut inventory = w.items.clone();
    inventory.push(build_helmet.clone());
    let set = armor_set(
        &CharacterId::new("1"),
        &[resolved(&build_helmet)],
        &inventory,
        &w.defs,
    );
    let ids: Vec<_> = set
        .iter()
        .map(|r| {
            r.inventory
                .as_ref()
                .unwrap()
                .instance_id
                .as_str()
                .to_owned()
        })
        .collect();
    assert_eq!(ids, ["9", "2", "3", "4", "5"]);
}

mod planner {
    use super::*;

    const BUCKETS: [i64; 8] = [
        1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
    ];
    fn characters() -> Vec<Character> {
        vec![Character {
            id: CharacterId::new("1"),
            name: None,
            class: Some("Hunter".into()),
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }]
    }
    fn plan(w: &World, build: &BuildDefinition, unlocked: &[u32]) -> BuildApplyPlan {
        let policy = EquipmentCapacityPolicy::new(
            DefinitionVersion::new("v1"),
            "fixture",
            BUCKETS.map(|b| (BucketHash::new(b), 9)).to_vec(),
        )
        .unwrap();
        let selected: Vec<_> = w.items.iter().map(resolved).collect();
        plan_build_items(
            &CharacterId::new("1"),
            &selected,
            &w.items,
            &w.defs,
            &characters(),
            &policy,
            &ApplyContext {
                armor_mods: Some(ModInput {
                    build,
                    unlocked,
                    catalogs: &w.catalogs,
                    fashion_catalogs: &[],
                    ornament_unlocks: &[],
                }),
                ..Default::default()
            },
        )
    }

    #[test]
    fn mod_insertions_form_their_own_phase_and_replace_the_deferred_marker() {
        let w = world();
        let plan = plan(&w, &build(&[2001], false), &[2001]);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].phase, ApplyPhase::Mods);
        assert_eq!(
            plan.steps[0].action,
            BuildAction::InsertPlug {
                item: ItemInstanceId::new("1"),
                character: CharacterId::new("1"),
                socket_index: 0,
                plug: ItemHash::new(2001),
            }
        );
        assert!(plan.not_applied.is_empty() && plan.mod_skipped.is_empty());
    }

    #[test]
    fn skipped_mods_and_fashion_are_listed_and_nothing_is_deferred() {
        let w = world();
        let mut wanted = build(&[2001], false);
        wanted.parameters.as_mut().unwrap().mods_by_bucket =
            Some([(BucketHash::new(ARMOR_BUCKETS[0]), vec![ItemHash::new(2002)])].into());
        let plan = plan(&w, &wanted, &[]);
        assert!(plan.steps.is_empty());
        assert_eq!(
            plan.mod_skipped,
            [
                ModSkip {
                    plug: ItemHash::new(2001),
                    reason: "mod_not_unlocked"
                },
                ModSkip {
                    plug: ItemHash::new(2002),
                    reason: "fashion_not_unlocked"
                }
            ]
        );
        assert!(plan.not_applied.is_empty());
    }

    #[test]
    fn builds_without_mods_plan_no_mod_steps() {
        let w = world();
        let plan = plan(&w, &BuildDefinition::default(), &[]);
        assert!(plan.steps.is_empty() && plan.not_applied.is_empty());
        assert!(plan.mod_skipped.is_empty() && plan.mods_in_place == 0);
    }
}
