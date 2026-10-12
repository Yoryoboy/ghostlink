//! DIM socket-override planning: loadout-apply.ts:272-299,980-1260 and
//! advanced-write-actions.ts:41-103 (commit 29c122e).
use destiny_core::{
    armor_catalog::*, build::BuildItem, build_analysis::ResolvedBuildItem, build_sockets::*,
    domain::ClassType, *,
};

const SUBCLASS: i64 = 3284755031;
const ARTIFACT: i64 = 1506418338;
const EMPTY_ASPECT: u32 = 2931483505;
const EMPTY_FRAGMENT: u32 = 3854296178;

fn plug(hash: i64, capacity: Option<i32>, cost_set: Option<u32>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: "Plug".into(),
        tier: None,
        item_type: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            aspect_capacity: capacity,
            insertion_material_requirement_hash: cost_set,
            ..Default::default()
        },
        non_transferrable: None,
        is_instance_item: Some(false),
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: None,
        stack_unique_label: None,
    }
}
fn entry(socket_type: u32, initial: u32, set: Option<u32>) -> DefinitionSocketEntry {
    DefinitionSocketEntry {
        socket_type_hash: socket_type,
        single_initial_item_hash: initial,
        reusable_plug_set_hash: set,
        plug_sources: Some(2),
        ..Default::default()
    }
}
fn category(hash: u32, indexes: &[u32]) -> DefinitionSocketCategory {
    DefinitionSocketCategory {
        socket_category_hash: hash,
        socket_indexes: indexes.to_vec(),
    }
}
fn container(hash: i64, bucket: i64, armor: ArmorDefinition) -> ItemDefinition {
    ItemDefinition {
        bucket_hash: Some(BucketHash::new(bucket)),
        class_type: Some(ClassType::Hunter),
        is_instance_item: Some(true),
        armor,
        ..plug(hash, None, None)
    }
}
/// Super (0), ability with a plug set but no initial plug (1), two aspects (2, 3),
/// three fragments (4, 5, 6). Socket types mirror their socket category.
fn subclass_definition() -> ItemDefinition {
    container(
        1,
        SUBCLASS,
        ArmorDefinition {
            socket_entries: vec![
                entry(10, 100, None),
                entry(11, 0, Some(700)),
                entry(12, EMPTY_ASPECT, Some(701)),
                entry(12, EMPTY_ASPECT, Some(701)),
                entry(13, EMPTY_FRAGMENT, Some(702)),
                entry(13, EMPTY_FRAGMENT, Some(702)),
                entry(13, EMPTY_FRAGMENT, Some(702)),
            ],
            socket_categories: vec![
                category(457473665, &[0]),
                category(309722977, &[1]),
                category(2047681910, &[2, 3]),
                category(271461480, &[4, 5, 6]),
            ],
            ..Default::default()
        },
    )
}
fn socket_type(hash: u32, category: u32) -> ArmorCatalogRecord {
    ArmorCatalogRecord::SocketType(SocketTypeDefinition {
        hash,
        socket_category_hash: category,
        always_randomize_sockets: false,
        plug_whitelist: vec![],
    })
}
fn plug_set(hash: u32, members: &[u32]) -> ArmorCatalogRecord {
    ArmorCatalogRecord::PlugSet(PlugSetDefinition {
        hash,
        reusable_plug_items: members
            .iter()
            .map(|m| PlugSetItem {
                plug_item_hash: *m,
                currently_can_roll: true,
            })
            .collect(),
    })
}
fn catalogs() -> Vec<ArmorCatalogRecord> {
    vec![
        socket_type(10, 457473665),
        socket_type(11, 309722977),
        socket_type(12, 2047681910),
        socket_type(13, 271461480),
        plug_set(700, &[200, 201]),
        plug_set(701, &[300, 301, 302, EMPTY_ASPECT]),
        plug_set(702, &[400, 401, 402, 403, EMPTY_FRAGMENT]),
    ]
}
fn socket(index: u8, plug: u32) -> IndexedSocket {
    IndexedSocket {
        plug_objectives: None,
        reusable_plugs: Some(vec![]),
        socket_index: index,
        plug_hash: Some(ItemHash::new(i64::from(plug))),
        enabled: true,
        visible: true,
    }
}
fn instance(hash: i64, location: ItemLocation, sockets: Vec<IndexedSocket>) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new("500"),
        item_hash: ItemHash::new(hash),
        location,
        equipped: true,
        locked: false,
        quantity: 1,
        power: None,
        transfer_status: None,
        energy: None,
        gear_tier: None,
        sockets,
        available_plugs: vec![],
        perks: vec![],
        plugs: vec![],
        stats: vec![],
    }
}
fn here() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("1"))
}
fn resolved(item: Option<ItemInstance>, overrides: &[(u16, u32)]) -> ResolvedBuildItem {
    ResolvedBuildItem {
        spec: BuildItem {
            instance_id: None,
            hash: item.as_ref().map_or(ItemHash::new(1), |i| i.item_hash),
            amount: None,
            socket_overrides: overrides
                .iter()
                .map(|(i, h)| (*i, ItemHash::new(i64::from(*h))))
                .collect(),
            crafted_date: None,
        },
        should_equip: true,
        inventory: item,
        class_compatible: Some(true),
        resolution_reason: None,
    }
}
fn character(class: &str) -> Vec<Character> {
    vec![Character {
        id: CharacterId::new("1"),
        name: None,
        class: Some(class.into()),
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    }]
}
fn subclass_plugs() -> Vec<ItemDefinition> {
    let mut defs = vec![
        subclass_definition(),
        plug(300, Some(2), None),
        plug(301, Some(1), None),
        plug(302, Some(1), None),
        plug(EMPTY_ASPECT.into(), None, None),
        plug(EMPTY_FRAGMENT.into(), None, None),
    ];
    for hash in [100, 200, 201, 400, 401, 402, 403] {
        defs.push(plug(hash, None, None));
    }
    defs
}
fn current_subclass() -> ItemInstance {
    instance(
        1,
        here(),
        vec![
            socket(0, 100),
            socket(1, 201),
            socket(2, 301),
            socket(3, EMPTY_ASPECT),
            socket(4, 402),
            socket(5, 401),
            socket(6, 403),
        ],
    )
}
fn plan_for(
    selected: &[ResolvedBuildItem],
    defs: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    class: &str,
) -> SocketPlan {
    plan_socket_overrides(
        &CharacterId::new("1"),
        selected,
        defs,
        &character(class),
        catalogs,
    )
}
fn inserts(plan: &SocketPlan) -> Vec<(u8, i64)> {
    plan.inserts
        .iter()
        .map(|i| (i.socket_index, i.plug.value()))
        .collect()
}

#[test]
fn subclass_overrides_follow_dim_category_order_and_shuffle_rules() {
    // Aspects 300 (+2) and 301 (+1) give three fragment slots. Ability 1 has no override,
    // so DIM patches in the first plug-set entry (200) (`loadout-apply.ts:283-299`).
    let spec = resolved(
        Some(current_subclass()),
        &[(0, 100), (2, 300), (3, 301), (4, 400), (5, 401)],
    );
    let plan = plan_for(&[spec], &subclass_plugs(), &catalogs(), "Hunter");
    // Aspect 301 stays where it is; the other aspect goes into the excess socket.
    // Fragment 401 stays; 400 takes the first excess socket; the leftover socket is
    // emptied (`loadout-apply.ts:1142-1192`).
    assert_eq!(
        inserts(&plan),
        [(1, 200), (3, 300), (4, 400), (6, i64::from(EMPTY_FRAGMENT))]
    );
    assert!(plan.skipped.is_empty());
    // Already-plugged requested overrides count as done without a request: the super
    // (override 0) and the kept aspect and fragment, reported by their override index.
    assert_eq!(plan.already_there, [(0, 0), (0, 3), (0, 5)]);
    assert!(
        plan.inserts
            .iter()
            .all(|i| i.item.as_str() == "500" && i.spec_index == 0)
    );
}

#[test]
fn excess_sockets_take_the_remaining_overrides_from_the_end_like_dim() {
    // Neither aspect is plugged: DIM pops the last requested aspect first
    // (`loadout-apply.ts:1171-1173`), so the order is deliberately reversed.
    let mut item = current_subclass();
    item.sockets[2] = socket(2, 302);
    let spec = resolved(Some(item), &[(2, 300), (3, 301)]);
    let plan = plan_for(&[spec], &subclass_plugs(), &catalogs(), "Hunter");
    let aspects: Vec<_> = inserts(&plan)
        .into_iter()
        .filter(|(socket, _)| (2..=3).contains(socket))
        .collect();
    assert_eq!(aspects, [(2, 301), (3, 300)]);
}

#[test]
fn fragments_beyond_the_aspect_capacity_are_left_alone() {
    // Only aspect 302 (+1): one fragment slot, so sockets 5 and 6 are never touched.
    let spec = resolved(Some(current_subclass()), &[(2, 302), (4, 400), (5, 401)]);
    let plan = plan_for(&[spec], &subclass_plugs(), &catalogs(), "Hunter");
    assert!(inserts(&plan).contains(&(4, 400)));
    assert!(inserts(&plan).iter().all(|(s, _)| *s != 5 && *s != 6));
}

#[test]
fn missing_aspect_overrides_fall_back_to_current_capacity_and_skip_untouched_categories() {
    // No aspects requested: capacity comes from the plugged aspects (301 = 1 slot), and
    // aspects themselves are not touched. Fragment 400 replaces the lone slot's plug.
    let spec = resolved(Some(current_subclass()), &[(4, 400)]);
    let plan = plan_for(&[spec], &subclass_plugs(), &catalogs(), "Hunter");
    assert_eq!(inserts(&plan), [(1, 200), (4, 400)]);
}

#[test]
fn no_overrides_means_no_socket_work() {
    let spec = resolved(Some(current_subclass()), &[]);
    let plan = plan_for(&[spec], &subclass_plugs(), &catalogs(), "Hunter");
    assert_eq!(plan, SocketPlan::default());
}

#[test]
fn dim_refuses_unusable_items_before_any_request() {
    let defs = subclass_plugs();
    let overrides = [(0, 100u32)];
    let cases = [
        (None, "Hunter", SocketSkipReason::Unresolved),
        (
            Some(current_subclass()),
            "Titan",
            SocketSkipReason::ClassMismatch,
        ),
        (
            Some(instance(1, ItemLocation::Vault, current_subclass().sockets)),
            "Hunter",
            SocketSkipReason::NotOnCharacter,
        ),
        (
            Some(instance(1, here(), vec![])),
            "Hunter",
            SocketSkipReason::SocketStateUnavailable,
        ),
        (
            Some(instance(999, here(), current_subclass().sockets)),
            "Hunter",
            SocketSkipReason::MissingDefinition,
        ),
    ];
    for (item, class, reason) in cases {
        let plan = plan_for(&[resolved(item, &overrides)], &defs, &catalogs(), class);
        assert!(plan.inserts.is_empty(), "{reason:?}");
        assert_eq!(
            plan.skipped,
            [SocketSkip {
                spec_index: 0,
                socket_index: None,
                reason
            }]
        );
    }
}

#[test]
fn only_free_insertions_are_planned() {
    // `canInsertForFree` (advanced-write-actions.ts:61-103): a plug with a material
    // cost, or replacing one that had a cost, is not free.
    let mut defs = subclass_plugs();
    defs.iter_mut()
        .find(|d| d.hash.value() == 400)
        .unwrap()
        .armor
        .insertion_material_requirement_hash = Some(900);
    defs.iter_mut()
        .find(|d| d.hash.value() == 403)
        .unwrap()
        .armor
        .insertion_material_requirement_hash = Some(900);
    let mut cats = catalogs();
    cats.push(ArmorCatalogRecord::MaterialRequirementSet(
        MaterialRequirementSetDefinition {
            hash: 900,
            materials: vec![MaterialRequirement {
                item_hash: 1,
                count: 5,
            }],
        },
    ));
    let spec = resolved(
        Some(current_subclass()),
        &[(2, 300), (3, 301), (4, 400), (5, 401)],
    );
    let plan = plan_for(&[spec], &defs, &cats, "Hunter");
    // 400 costs materials; the emptying of socket 6 replaces costly 403.
    assert_eq!(inserts(&plan), [(1, 200), (3, 300)]);
    assert_eq!(
        plan.skipped,
        [SocketSkip {
            spec_index: 0,
            socket_index: Some(4),
            reason: SocketSkipReason::NotFree
        }]
    );
    // A zero count is not a cost, and an unknown requirement set is not guessed.
    let mut free = cats.clone();
    free.retain(|c| !matches!(c, ArmorCatalogRecord::MaterialRequirementSet(_)));
    free.push(ArmorCatalogRecord::MaterialRequirementSet(
        MaterialRequirementSetDefinition {
            hash: 900,
            materials: vec![MaterialRequirement {
                item_hash: 1,
                count: 0,
            }],
        },
    ));
    let spec = resolved(Some(current_subclass()), &[(4, 400)]);
    assert!(
        inserts(&plan_for(
            std::slice::from_ref(&spec),
            &defs,
            &free,
            "Hunter"
        ))
        .contains(&(4, 400))
    );
    let mut unknown = cats;
    unknown.retain(|c| !matches!(c, ArmorCatalogRecord::MaterialRequirementSet(_)));
    let plan = plan_for(&[spec], &defs, &unknown, "Hunter");
    assert!(plan.inserts.iter().all(|i| i.socket_index != 4));
    assert_eq!(
        plan.skipped[0].reason,
        SocketSkipReason::InsertionCostUnavailable
    );
}

#[test]
fn unknown_plug_definitions_and_capacity_are_typed_skips() {
    let mut defs = subclass_plugs();
    defs.retain(|d| d.hash.value() != 400);
    let spec = resolved(Some(current_subclass()), &[(2, 300), (3, 301), (4, 400)]);
    let plan = plan_for(std::slice::from_ref(&spec), &defs, &catalogs(), "Hunter");
    assert!(plan.inserts.iter().all(|i| i.socket_index != 4));
    assert_eq!(
        plan.skipped[0].reason,
        SocketSkipReason::PlugDefinitionUnavailable
    );
    // Without a plug definition for the current aspect, the fallback capacity is unknown.
    let mut defs = subclass_plugs();
    defs.retain(|d| d.hash.value() != 301);
    let spec = resolved(Some(current_subclass()), &[(4, 400)]);
    let plan = plan_for(&[spec], &defs, &catalogs(), "Hunter");
    assert!(plan.inserts.iter().all(|i| i.socket_index != 4));
    assert_eq!(
        plan.skipped[0].reason,
        SocketSkipReason::FragmentCapacityUnknown
    );
}

/// Artifact with two perk sockets drawing from set 800 (A=1001, B=1002) and an
/// optional second set that lacks A.
fn artifact(second_set: &[u32]) -> (ItemDefinition, Vec<ArmorCatalogRecord>) {
    let definition = container(
        2,
        ARTIFACT,
        ArmorDefinition {
            socket_entries: vec![entry(20, 5000, Some(800)), entry(20, 5000, Some(801))],
            socket_categories: vec![category(1, &[0, 1])],
            ..Default::default()
        },
    );
    (
        definition,
        vec![
            socket_type(20, 1),
            plug_set(800, &[1001, 1002]),
            plug_set(801, second_set),
        ],
    )
}
fn artifact_plugs(definition: ItemDefinition) -> Vec<ItemDefinition> {
    vec![
        definition,
        plug(1001, None, None),
        plug(1002, None, None),
        plug(5000, None, None),
    ]
}

#[test]
fn artifact_swaps_are_kept_when_the_other_socket_accepts_the_plug() {
    // Overrides {0: A, 1: B} but the item has {0: B, 1: A}: both sets take both plugs,
    // so DIM keeps the plugs where they are (`loadout-apply.ts:1040-1104`).
    let (definition, cats) = artifact(&[1001, 1002]);
    let item = instance(2, here(), vec![socket(0, 1002), socket(1, 1001)]);
    let spec = resolved(Some(item), &[(0, 1001), (1, 1002)]);
    let plan = plan_for(&[spec], &artifact_plugs(definition), &cats, "Hunter");
    assert!(plan.inserts.is_empty());
    assert_eq!(plan.already_there.len(), 2);
}

#[test]
fn artifact_empties_a_socket_whose_plug_cannot_move() {
    // The second socket does not accept A, so socket 0 is emptied and B goes in later.
    let (definition, cats) = artifact(&[1002]);
    let item = instance(2, here(), vec![socket(0, 1002), socket(1, 1001)]);
    let spec = resolved(Some(item), &[(0, 1001), (1, 1002)]);
    let plan = plan_for(&[spec], &artifact_plugs(definition), &cats, "Hunter");
    assert_eq!(inserts(&plan), [(0, 5000), (0, 1002)]);
}

#[test]
fn artifact_override_on_a_missing_socket_is_skipped() {
    let (definition, cats) = artifact(&[1001, 1002]);
    let item = instance(2, here(), vec![socket(0, 1002), socket(1, 1001)]);
    let spec = resolved(Some(item), &[(7, 1001)]);
    let plan = plan_for(&[spec], &artifact_plugs(definition), &cats, "Hunter");
    assert!(plan.inserts.is_empty());
}

mod in_plan {
    use super::*;
    use destiny_core::build_apply::*;

    const WEAPON_BUCKET: i64 = 1498876634;
    const BUCKETS: [i64; 8] = [
        1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
    ];

    /// A weapon with one generic socket drawing from set 800 (1001, 1002).
    fn weapon() -> (ItemDefinition, Vec<ItemDefinition>, Vec<ArmorCatalogRecord>) {
        let definition = ItemDefinition {
            class_type: Some(ClassType::Any),
            slot: domain::ItemSlot::from_equipment_bucket(BucketHash::new(WEAPON_BUCKET)),
            ..container(
                3,
                WEAPON_BUCKET,
                ArmorDefinition {
                    unique_label: Some(String::new()),
                    socket_entries: vec![entry(20, 5000, Some(800))],
                    socket_categories: vec![category(1, &[0])],
                    ..Default::default()
                },
            )
        };
        let defs = vec![
            definition.clone(),
            plug(1001, None, None),
            plug(1002, None, None),
            plug(5000, None, None),
        ];
        (
            definition,
            defs,
            vec![socket_type(20, 1), plug_set(800, &[1001, 1002])],
        )
    }
    fn plan_with(
        item: ItemInstance,
        context: &ApplyContext,
        defs: &[ItemDefinition],
    ) -> BuildApplyPlan {
        let policy = EquipmentCapacityPolicy::new(
            DefinitionVersion::new("v1"),
            "fixture",
            BUCKETS.map(|b| (BucketHash::new(b), 9)).to_vec(),
        )
        .unwrap();
        let selected = [resolved(Some(item.clone()), &[(0, 1002)])];
        plan_build_items(
            &CharacterId::new("1"),
            &selected,
            &[item],
            defs,
            &character("Hunter"),
            &policy,
            context,
        )
    }
    fn vault() -> VaultEvidence {
        VaultEvidence::single(
            &BUCKETS.map(BucketHash::new),
            BucketHash::new(138197802),
            50,
        )
    }

    #[test]
    fn plug_insertions_follow_equips_and_wait_for_the_moves_they_depend_on() {
        let (_, defs, cats) = weapon();
        let mut item = instance(3, ItemLocation::Vault, vec![socket(0, 1001)]);
        item.equipped = false;
        let vault = vault();
        let context = ApplyContext {
            vault: Some(&vault),
            catalogs: Some(&cats),
            ..Default::default()
        };
        let plan = plan_with(item, &context, &defs);
        // Move and inline equip, then the insertion.
        let phases: Vec<_> = plan.steps.iter().map(|s| s.phase).collect();
        assert_eq!(
            phases,
            [ApplyPhase::Move, ApplyPhase::Move, ApplyPhase::Sockets]
        );
        // The weapon is judged where the plan leaves it: on the target character.
        let BuildAction::InsertPlug {
            item,
            character,
            socket_index,
            plug,
        } = &plan.steps[2].action
        else {
            panic!("plug insertion expected");
        };
        assert_eq!(
            (
                item.as_str(),
                character.as_str(),
                *socket_index,
                plug.value()
            ),
            ("500", "1", 0, 1002)
        );
        assert_eq!(plan.steps[2].needs, [0, 1]);
        assert!(plan.not_applied.is_empty() && plan.socket_skipped.is_empty());
    }

    #[test]
    fn overrides_without_catalogs_are_reported_not_guessed() {
        let (_, defs, _) = weapon();
        let item = instance(3, here(), vec![socket(0, 1001)]);
        let plan = plan_with(item, &ApplyContext::default(), &defs);
        assert!(plan.steps.iter().all(|s| s.phase != ApplyPhase::Sockets));
        assert_eq!(plan.not_applied, ["socket_overrides_evidence_unavailable"]);
        assert!(plan.unknown.contains(&"socket_catalogs"));
    }

    #[test]
    fn an_override_already_in_place_needs_no_request() {
        let (_, defs, cats) = weapon();
        let item = instance(3, here(), vec![socket(0, 1002)]);
        let context = ApplyContext {
            catalogs: Some(&cats),
            ..Default::default()
        };
        let plan = plan_with(item, &context, &defs);
        assert!(plan.not_applied.is_empty());
        // Plug 1002 is already in place: nothing to dispatch.
        assert!(plan.steps.is_empty());
        assert_eq!(plan.socket_already_there, [(0, 0)]);
    }
}
