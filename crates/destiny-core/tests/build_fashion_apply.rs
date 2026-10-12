//! DIM fashion application: `applyLoadoutMods` fashion patch and `equipModsToItem`
//! (`loadout-apply.ts:308-338,1365-1405`), `insertPlug` and `checkIrreversiblePlugging`
//! (`advanced-write-actions.ts:105-175`), commit 29c122e. Fakes only.
use destiny_core::{
    armor_catalog::*,
    build::*,
    build_analysis::ResolvedBuildItem,
    build_apply::*,
    build_fashion_apply::*,
    build_mod_apply::*,
    domain::{ClassType, ReusablePlug},
    mod_assignment::ARMOR_BUCKETS,
    *,
};

const SHADER_A: u32 = 3001;
const SHADER_B: u32 = 3002;
const ORNAMENT: u32 = 3101;
const OTHER_ORNAMENT: u32 = 3102;
const DEFAULT_SHADER: u32 = 4248210736;
const DEFAULT_ORNAMENT: u32 = 702981643;
const SHADER_CATEGORY: u32 = 41;
const COSMETICS: u32 = 1926152773;
const SUBTYPE_SHADER: u8 = 20;
const SUBTYPE_ORNAMENT: u8 = 21;

fn plug(hash: u32, sub_type: u8, categories: &[u32], requirement: Option<u32>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(i64::from(hash)),
        bucket_hash: None,
        name: format!("Plug {hash}"),
        tier: Some("Legendary".into()),
        item_type: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            plug_category_hash: Some(7),
            item_category_hashes: categories.to_vec(),
            item_sub_type_code: Some(sub_type),
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
fn socket(index: u8, plugged: u32, others: &[(u32, bool)]) -> IndexedSocket {
    let mut reusable: Vec<_> = others
        .iter()
        .map(|(hash, enabled)| ReusablePlug {
            plug_hash: ItemHash::new(i64::from(*hash)),
            enabled: *enabled,
            can_insert: *enabled,
        })
        .collect();
    reusable.push(ReusablePlug {
        plug_hash: ItemHash::new(i64::from(plugged)),
        enabled: true,
        can_insert: true,
    });
    IndexedSocket {
        plug_objectives: None,
        reusable_plugs: Some(reusable),
        socket_index: index,
        plug_hash: Some(ItemHash::new(i64::from(plugged))),
        enabled: true,
        visible: true,
    }
}
fn entry(socket_type: u32, set: u32) -> DefinitionSocketEntry {
    DefinitionSocketEntry {
        socket_type_hash: socket_type,
        reusable_plug_set_hash: Some(set),
        ..Default::default()
    }
}

#[derive(Clone)]
struct World {
    defs: Vec<ItemDefinition>,
    catalogs: Vec<ArmorCatalogRecord>,
    items: Vec<ItemInstance>,
}
/// Five armor pieces (ids 1-5, bucket order). Socket 0 is a mod socket, 1 the shader
/// socket (default shader plugged) and 2 the ornament socket (default ornament plugged).
fn world() -> World {
    let mut defs = vec![
        plug(DEFAULT_SHADER, SUBTYPE_SHADER, &[SHADER_CATEGORY], None),
        plug(SHADER_A, SUBTYPE_SHADER, &[SHADER_CATEGORY], None),
        plug(SHADER_B, SUBTYPE_SHADER, &[SHADER_CATEGORY], None),
        // The default armor ornament is itself marked as a shader (DIM comment).
        plug(DEFAULT_ORNAMENT, SUBTYPE_ORNAMENT, &[SHADER_CATEGORY], None),
        plug(ORNAMENT, SUBTYPE_ORNAMENT, &[], None),
        plug(OTHER_ORNAMENT, SUBTYPE_ORNAMENT, &[], None),
    ];
    let mut items = vec![];
    for (index, bucket) in ARMOR_BUCKETS.into_iter().enumerate() {
        let mut armor = plug(index as u32 + 11, 0, &[], None);
        armor.armor.plug_category_hash = None;
        armor.armor.item_sub_type_code = None;
        armor.bucket_hash = Some(BucketHash::new(bucket));
        armor.class_type = Some(ClassType::Hunter);
        armor.armor.socket_entries = vec![entry(99, 800), entry(98, 810), entry(97, 811)];
        armor.armor.socket_categories = vec![DefinitionSocketCategory {
            socket_category_hash: COSMETICS,
            socket_indexes: vec![1, 2],
        }];
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
            energy: None,
            gear_tier: None,
            transfer_status: Some(0),
            sockets: vec![
                socket(0, 1, &[]),
                socket(1, DEFAULT_SHADER, &[(SHADER_A, true), (SHADER_B, true)]),
                socket(2, DEFAULT_ORNAMENT, &[(ORNAMENT, true)]),
            ],
        });
        defs.push(armor);
    }
    let set = |hash, members: &[u32]| {
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash,
            reusable_plug_items: members
                .iter()
                .map(|plug_item_hash| PlugSetItem {
                    plug_item_hash: *plug_item_hash,
                    currently_can_roll: true,
                })
                .collect(),
        })
    };
    let kind = |hash| {
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        })
    };
    let catalogs = vec![
        kind(98),
        kind(97),
        set(810, &[DEFAULT_SHADER, SHADER_A, SHADER_B]),
        set(811, &[DEFAULT_ORNAMENT, ORNAMENT, OTHER_ORNAMENT]),
    ];
    World {
        defs,
        catalogs,
        items,
    }
}
fn here() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("1"))
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
fn fashion(entries: &[(usize, &[u32])]) -> BuildDefinition {
    BuildDefinition {
        parameters: Some(BuildParameters {
            mods_by_bucket: Some(
                entries
                    .iter()
                    .map(|(bucket, hashes)| {
                        (
                            BucketHash::new(ARMOR_BUCKETS[*bucket]),
                            hashes
                                .iter()
                                .map(|h| ItemHash::new(i64::from(*h)))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}
impl World {
    fn clone_defs_with(&self, extra: ItemDefinition) -> World {
        let mut world = self.clone();
        world.defs.push(extra);
        world
    }
    fn plan(&self, build: &BuildDefinition, unlocked: &[u32], ornaments: &[u32]) -> ModPlan {
        self.plan_with(build, unlocked, ornaments, &self.items)
    }
    fn plan_with(
        &self,
        build: &BuildDefinition,
        unlocked: &[u32],
        ornaments: &[u32],
        items: &[ItemInstance],
    ) -> ModPlan {
        let armor: Vec<_> = items.iter().map(resolved).collect();
        let input = ModInput {
            build,
            unlocked,
            catalogs: &[],
            fashion_catalogs: &self.catalogs,
            ornament_unlocks: ornaments,
        };
        plan_fashion(&input, &self.defs, &CharacterId::new("1"), &armor)
    }
}
fn inserts(plan: &ModPlan) -> Vec<(&str, u8, i64)> {
    plan.inserts
        .iter()
        .map(|i| (i.item.as_str(), i.socket_index, i.plug.value()))
        .collect()
}
fn skipped(plan: &ModPlan) -> Vec<(i64, &'static str)> {
    plan.skipped
        .iter()
        .map(|s| (s.plug.value(), s.reason))
        .collect()
}

#[test]
fn shaders_and_ornaments_go_into_the_socket_whose_plug_set_holds_them() {
    // `plugFitsIntoSocket` picks the first fitting socket (`loadout-apply.ts:1365-1380`).
    let w = world();
    let wanted = fashion(&[(0, &[SHADER_B, ORNAMENT]), (1, &[SHADER_A])]);
    let plan = w.plan(&wanted, &[SHADER_A, SHADER_B, ORNAMENT], &[]);
    assert_eq!(
        inserts(&plan),
        [
            ("1", 1, SHADER_B.into()),
            ("1", 2, ORNAMENT.into()),
            ("2", 1, SHADER_A.into())
        ]
    );
    assert!(plan.skipped.is_empty() && plan.not_applied.is_empty());
}

#[test]
fn fashion_that_is_not_unlocked_is_dropped_but_defaults_and_exotic_ornaments_pass() {
    // `checkMod` (`loadout-apply.ts:308-320`).
    let w = world();
    let wanted = fashion(&[(0, &[SHADER_B, ORNAMENT, DEFAULT_ORNAMENT, OTHER_ORNAMENT])]);
    let plan = w.plan(&wanted, &[], &[OTHER_ORNAMENT]);
    assert_eq!(inserts(&plan), [("1", 2, OTHER_ORNAMENT.into())]);
    assert_eq!(
        skipped(&plan),
        [
            (SHADER_B.into(), "fashion_not_unlocked"),
            (ORNAMENT.into(), "fashion_not_unlocked")
        ]
    );
    // The default ornament is already plugged (no request) and counts as in place.
    assert_eq!(plan.in_place, 1);
    let wanted = fashion(&[(0, &[DEFAULT_SHADER])]);
    let mut items = w.items.clone();
    items[0].sockets[1] = socket(1, SHADER_A, &[(SHADER_A, true)]);
    let plan = w.plan_with(&wanted, &[SHADER_A], &[], &items);
    assert_eq!(inserts(&plan), [("1", 1, DEFAULT_SHADER.into())]);
}

#[test]
fn a_plug_already_in_its_socket_needs_no_request() {
    let w = world();
    let mut items = w.items.clone();
    items[0].sockets[1] = socket(1, SHADER_B, &[]);
    let plan = w.plan_with(&fashion(&[(0, &[SHADER_B])]), &[SHADER_B], &[], &items);
    assert!(plan.inserts.is_empty() && plan.skipped.is_empty());
    assert_eq!(plan.in_place, 1);
}

#[test]
fn an_ornament_made_from_the_same_item_swaps_to_the_socket_default() {
    // You cannot plug an item's own transmog ornament; DIM plugs the default one
    // instead (`loadout-apply.ts:1489-1500`, `advanced-write-actions.ts:144-151`).
    let mut w = world();
    let helmet = w.defs.iter().position(|d| d.hash.value() == 11).unwrap();
    w.defs[helmet].armor.plug_category_hash = Some(7);
    if let Some(ArmorCatalogRecord::PlugSet(set)) = w.catalogs.get_mut(3) {
        set.reusable_plug_items.push(PlugSetItem {
            plug_item_hash: 11,
            currently_can_roll: true,
        });
    }
    let mut items = w.items.clone();
    items[0].sockets[2] = socket(2, ORNAMENT, &[(ORNAMENT, true)]);
    let plan = w.plan_with(&fashion(&[(0, &[11])]), &[11], &[], &items);
    assert_eq!(inserts(&plan), [("1", 2, DEFAULT_ORNAMENT.into())]);
    assert!(plan.skipped.is_empty());
}

#[test]
fn a_shader_the_player_cannot_get_back_is_never_overwritten() {
    // `checkIrreversiblePlugging` (`advanced-write-actions.ts:105-135`): shaders can be
    // overwritten while locked, so a non-default shader that is neither unlocked in the
    // plug sets nor enabled on the item stays.
    let w = world();
    let mut items = w.items.clone();
    let reusable = |a_enabled| {
        Some(
            [(SHADER_A, a_enabled), (SHADER_B, true)]
                .map(|(hash, enabled)| ReusablePlug {
                    plug_hash: ItemHash::new(i64::from(hash)),
                    enabled,
                    can_insert: enabled,
                })
                .to_vec(),
        )
    };
    items[0].sockets[1] = socket(1, SHADER_A, &[]);
    items[0].sockets[1].reusable_plugs = reusable(false);
    let wanted = fashion(&[(0, &[SHADER_B])]);
    let plan = w.plan_with(&wanted, &[SHADER_B], &[], &items);
    assert!(plan.inserts.is_empty());
    assert_eq!(skipped(&plan), [(SHADER_B.into(), "irreversible_shader")]);
    // Unlocked in the plug sets: the shader can be plugged back, so it is replaced.
    let plan = w.plan_with(&wanted, &[SHADER_A, SHADER_B], &[], &items);
    assert_eq!(inserts(&plan), [("1", 1, SHADER_B.into())]);
    // Enabled among the item's own reusable plugs: also replaced.
    items[0].sockets[1].reusable_plugs = reusable(true);
    let plan = w.plan_with(&wanted, &[SHADER_B], &[], &items);
    assert_eq!(inserts(&plan), [("1", 1, SHADER_B.into())]);
    // Present but disabled on the item is not enough.
    items[0].sockets[1].reusable_plugs = reusable(false);
    let plan = w.plan_with(&wanted, &[SHADER_B], &[], &items);
    assert_eq!(skipped(&plan), [(SHADER_B.into(), "irreversible_shader")]);
}

#[test]
fn the_default_shader_and_default_ornaments_are_never_protected() {
    let w = world();
    let mut items = w.items.clone();
    // Default shader plugged and not unlocked anywhere; the default ornament carries the
    // shader category too ("for some reason the default armor ornament is a shader").
    items[0].sockets[1] = socket(1, DEFAULT_SHADER, &[]);
    items[0].sockets[2] = socket(2, DEFAULT_ORNAMENT, &[]);
    let wanted = fashion(&[(0, &[SHADER_B, ORNAMENT])]);
    let plan = w.plan_with(&wanted, &[SHADER_B, ORNAMENT], &[], &items);
    assert_eq!(
        inserts(&plan),
        [("1", 1, SHADER_B.into()), ("1", 2, ORNAMENT.into())]
    );
}

#[test]
fn an_unprotected_non_shader_plug_is_replaced_freely() {
    let w = world();
    let mut items = w.items.clone();
    items[0].sockets[2] = socket(2, OTHER_ORNAMENT, &[]);
    let plan = w.plan_with(&fashion(&[(0, &[ORNAMENT])]), &[ORNAMENT], &[], &items);
    assert_eq!(inserts(&plan), [("1", 2, ORNAMENT.into())]);
}

#[test]
fn fashion_without_a_fitting_socket_or_armor_piece_is_reported_never_sent() {
    let w = world();
    // The ornament is in no plug set of the item's sockets; bucket 4 has no piece in the set.
    let mut missing = w.clone_defs_with(plug(3999, SUBTYPE_ORNAMENT, &[], None));
    missing.items.truncate(2);
    let wanted = fashion(&[(0, &[3999]), (4, &[SHADER_A])]);
    let plan = missing.plan(&wanted, &[3999, SHADER_A], &[]);
    assert!(plan.inserts.is_empty());
    assert_eq!(
        skipped(&plan),
        [
            (3999, "fashion_unassigned"),
            (SHADER_A.into(), "fashion_no_armor")
        ]
    );
}

#[test]
fn only_free_insertions_are_planned_for_fashion() {
    let mut w = world();
    w.defs
        .push(plug(3200, SUBTYPE_SHADER, &[SHADER_CATEGORY], Some(900)));
    if let Some(ArmorCatalogRecord::PlugSet(set)) = w.catalogs.get_mut(2) {
        set.reusable_plug_items.push(PlugSetItem {
            plug_item_hash: 3200,
            currently_can_roll: true,
        });
    }
    w.catalogs.push(ArmorCatalogRecord::MaterialRequirementSet(
        MaterialRequirementSetDefinition {
            hash: 900,
            materials: vec![MaterialRequirement {
                item_hash: 1,
                count: 5,
            }],
        },
    ));
    let plan = w.plan(&fashion(&[(0, &[3200])]), &[3200], &[]);
    assert!(plan.inserts.is_empty());
    assert_eq!(skipped(&plan), [(3200, "not_free")]);
}

#[test]
fn armor_off_the_target_character_is_skipped_per_plug() {
    let w = world();
    let mut items = w.items.clone();
    items[0].location = ItemLocation::Vault;
    let plan = w.plan_with(&fashion(&[(0, &[SHADER_B])]), &[SHADER_B], &[], &items);
    assert!(plan.inserts.is_empty());
    assert_eq!(
        skipped(&plan),
        [(SHADER_B.into(), "not_on_target_character")]
    );
}

#[test]
fn builds_without_fashion_plan_nothing() {
    let w = world();
    let plan = w.plan(&BuildDefinition::default(), &[], &[]);
    assert_eq!(plan, ModPlan::default());
    let plan = w.plan(&fashion(&[]), &[], &[]);
    assert_eq!(plan, ModPlan::default());
}

#[test]
fn exotic_ornament_unlocks_come_from_exotic_armor_cosmetic_sockets_only() {
    // `unlockedExoticOrnamentsSelector` (`inventory/selectors.ts:372-389`).
    let mut w = world();
    for (index, tier) in [(0, "Exotic"), (1, "Legendary")] {
        let hash = w.items[index].item_hash;
        w.defs.iter_mut().find(|d| d.hash == hash).unwrap().tier = Some(tier.into());
    }
    // Exotic helmet: an ornament the account owns, one it does not, and the default.
    w.items[0].sockets[2].reusable_plugs = Some(
        [
            (ORNAMENT, true),
            (OTHER_ORNAMENT, false),
            (DEFAULT_ORNAMENT, true),
        ]
        .map(|(hash, can_insert)| ReusablePlug {
            plug_hash: ItemHash::new(i64::from(hash)),
            enabled: can_insert,
            can_insert,
        })
        .to_vec(),
    );
    // A legendary piece never counts, and neither does a non-cosmetic socket.
    w.items[1].sockets[2] = socket(2, DEFAULT_ORNAMENT, &[(OTHER_ORNAMENT, true)]);
    w.items[0].sockets[0] = socket(0, 1, &[(SHADER_A, true)]);
    // Like DIM, every cosmetic socket counts (the shader socket of the same piece too),
    // minus the default ornaments.
    assert_eq!(
        exotic_ornament_unlocks(&w.items, &w.defs),
        [SHADER_A, SHADER_B, ORNAMENT, DEFAULT_SHADER]
    );
}

#[test]
fn fashion_steps_follow_the_mod_steps_in_the_mods_phase_and_replace_the_deferred_marker() {
    let w = world();
    let wanted = fashion(&[(0, &[SHADER_B])]);
    let selected: Vec<_> = w.items.iter().map(resolved).collect();
    let policy = EquipmentCapacityPolicy::new(
        DefinitionVersion::new("v1"),
        "fixture",
        ARMOR_BUCKETS
            .iter()
            .chain(&[1498876634, 2465295065, 953998645])
            .map(|b| (BucketHash::new(*b), 10))
            .collect(),
    )
    .unwrap();
    let characters = vec![Character {
        id: CharacterId::new("1"),
        name: None,
        class: Some("Hunter".into()),
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    }];
    let plan = plan_build_items(
        &CharacterId::new("1"),
        &selected,
        &w.items,
        &w.defs,
        &characters,
        &policy,
        &ApplyContext {
            armor_mods: Some(ModInput {
                build: &wanted,
                unlocked: &[SHADER_B],
                catalogs: &[],
                fashion_catalogs: &w.catalogs,
                ornament_unlocks: &[],
            }),
            ..Default::default()
        },
    );
    assert!(plan.not_applied.is_empty(), "{:?}", plan.not_applied);
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].phase, ApplyPhase::Mods);
    assert_eq!(
        plan.steps[0].action,
        BuildAction::InsertPlug {
            item: ItemInstanceId::new("1"),
            character: CharacterId::new("1"),
            socket_index: 1,
            plug: ItemHash::new(SHADER_B.into()),
        }
    );
}
