use destiny_core::domain::ItemSlot;
use destiny_core::domain::{ClassType, DamageType, PerkSummary, StatSummary};
use destiny_core::query::{
    self, DefinitionSearchQuery, InstanceSearchQuery, ItemHashFilter, LocationFilter, SearchTerm,
};
use destiny_core::search_query::{ItemView, Lowering, compile, lower, matches, referenced_stats};
use destiny_core::{CharacterId, PerkHash, StatDefinition};
use destiny_core::{
    DefinitionVersion, ItemDefinition, ItemHash, ItemInstance, ItemInstanceId, ItemLocation,
};
use std::collections::BTreeMap;

fn definition() -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(1),
        bucket_hash: None,
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        name: " Sun  Shot ".into(),
        item_type: Some("Hand Cannon".into()),
        tier: Some("Exotic".into()),
        element: Some("solar".into()),
        slot: Some(ItemSlot::Kinetic),
        class_type: None,
        plug_category: None,
        tags: vec!["Favorite".into()],
        version: DefinitionVersion::new("1"),
    }
}
fn base_query() -> InstanceSearchQuery {
    InstanceSearchQuery {
        offset: 0,
        sort: None,
        include_available: false,
        item_hashes: ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        character_id: None,
        location: None,
        equipped: None,
        definitions: None,
        min_power: None,
        stats: vec![],
        limit: 1000,
    }
}
fn base_definitions() -> DefinitionSearchQuery {
    DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 1000,
    }
}
fn term(text: &str) -> SearchTerm {
    SearchTerm::new(text).unwrap()
}
fn instance() -> ItemInstance {
    ItemInstance {
        energy: None,
        gear_tier: None,
        sockets: Vec::new(),
        transfer_status: None,
        available_plugs: vec![],
        instance_id: ItemInstanceId::new("1"),
        item_hash: ItemHash::new(1),
        location: ItemLocation::Vault,
        quantity: 1,
        equipped: false,
        locked: true,
        power: Some(2001),
        perks: vec![],
        plugs: vec![],
        stats: vec![],
    }
}
/// Matrix includes both equipment categories, unknown/mismatched definitions,
/// all location classes, power/stat absence and alternate-plug policy.
#[test]
fn leaf_parity_and_prefilter_superset_matrix() {
    let mut weapon = definition();
    weapon.class_type = Some(ClassType::Hunter);
    let mut armor = weapon.clone();
    armor.slot = Some(ItemSlot::Helmet);
    let mut unknown = weapon.clone();
    unknown.slot = None;
    unknown.item_type = None;
    unknown.tier = None;
    unknown.element = None;
    unknown.class_type = None;
    unknown.tags.clear();
    let mut mismatched = weapon.clone();
    mismatched.hash = ItemHash::new(2);
    let plug = |name: &str, category: &str| {
        let mut d = definition();
        d.name = name.into();
        d.plug_category = Some(category.into());
        d
    };
    let selected = vec![
        plug("Dragonfly", "frames"),
        plug("Harmonic Siphon", "enhancements.v2_head"),
        plug("Bulwark", "armor_archetypes"),
        plug("Empty Mod Socket", "enhancements.v2_head"),
        plug("Dragonfly", "shader"),
    ];
    let available = vec![
        plug("Rampage", "frames"),
        plug("Targeting", "enhancements.v2_head"),
        plug("Specialist", "armor_archetypes"),
    ];
    let characters = BTreeMap::from([
        ("hunter".into(), CharacterId::new("123")),
        ("123".into(), CharacterId::new("123")),
    ]);
    let stat_defs = vec![
        StatDefinition::new(1, "Mobility".into()).unwrap(),
        StatDefinition::new(2, "Mobility".into()).unwrap(),
    ];
    for definition in [
        Some(&weapon),
        Some(&armor),
        Some(&unknown),
        Some(&mismatched),
        None,
    ] {
        for location in [
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new("123")),
            ItemLocation::Character(CharacterId::new("456")),
            ItemLocation::Postmaster(CharacterId::new("123")),
        ] {
            for power in [None, Some(0), Some(1999), Some(2000), Some(2001)] {
                for include_available in [false, true] {
                    let mut item = instance();
                    item.location = location.clone();
                    item.power = power;
                    item.equipped = power == Some(2000);
                    item.locked = power.is_some();
                    item.perks = vec![PerkSummary {
                        perk_hash: PerkHash::new(10),
                        active: false,
                        visible: false,
                    }];
                    let stats = if power.is_some() {
                        item.stats = vec![
                            StatSummary {
                                stat_hash: 1,
                                value: 20,
                            },
                            StatSummary {
                                stat_hash: 2,
                                value: 40,
                            },
                        ];
                        BTreeMap::from([("mobility".into(), vec![20, 40])])
                    } else {
                        BTreeMap::new()
                    };
                    let view = ItemView {
                        instance: &item,
                        definition,
                        socketed: &selected,
                        available: &available,
                        include_available,
                        stats: &stats,
                        characters: &characters,
                    };
                    let paired = definition.filter(|d| d.hash == item.item_hash);
                    let mut cases: Vec<(&str, InstanceSearchQuery)> = vec![];
                    for text in [
                        "sun",
                        "name:sun",
                        "type:\"hand cannon\"",
                        "tag:favorite",
                        "is:exotic",
                        "is:solar",
                        "is:kineticslot",
                        "is:hunter",
                    ] {
                        let mut q = base_query();
                        let mut d = base_definitions();
                        match text {
                            "sun" | "name:sun" => d.text = Some(term("sun")),
                            "type:\"hand cannon\"" => d.item_type = Some(term("hand cannon")),
                            "tag:favorite" => d.tags.push(term("favorite")),
                            "is:exotic" => d.tier = Some(term("exotic")),
                            "is:solar" => d.element = Some(DamageType::Solar),
                            "is:kineticslot" => d.slot = Some(ItemSlot::Kinetic),
                            _ => d.class_type = Some(ClassType::Hunter),
                        }
                        q.definitions = Some(Box::new(d));
                        cases.push((text, q));
                    }
                    for text in [
                        "hash:1",
                        "perkhash:10",
                        "is:invault",
                        "is:ininventory",
                        "is:equipped",
                        "character:hunter",
                        "character:123",
                        "exactperk:dragonfly",
                        "mod:\"harmonic siphon\"",
                        "archetype:bulwark",
                        "stat:mobility:>30",
                        "power:>=2000",
                    ] {
                        let mut q = base_query();
                        q.include_available = include_available;
                        match text {
                            "hash:1" => {
                                q.item_hashes = ItemHashFilter::AnyOf(vec![ItemHash::new(1)])
                            }
                            "perkhash:10" => q.perk_hash = Some(PerkHash::new(10)),
                            "is:invault" => q.location = Some(LocationFilter::Vault),
                            "is:ininventory" => q.location = Some(LocationFilter::Character),
                            "is:equipped" => q.equipped = Some(true),
                            "character:hunter" | "character:123" => {
                                q.character_id = Some(CharacterId::new("123"))
                            }
                            "exactperk:dragonfly" => q.perk_name = Some(vec![term("dragonfly")]),
                            "mod:\"harmonic siphon\"" => {
                                q.mod_name = Some(vec![term("harmonic siphon")])
                            }
                            "archetype:bulwark" => q.archetype = Some(term("bulwark")),
                            "stat:mobility:>30" => q.stats.push("mobility>30".parse().unwrap()),
                            _ => q.min_power = Some(2000),
                        }
                        cases.push((text, q));
                    }
                    for (text, q) in cases {
                        let expected =
                            query::instance_matches_with_definition(&q, &item, definition)
                                && query::instance_matches_available_names(
                                    &q, &selected, &available,
                                )
                                && query::instance_matches_stats(&q, &item, &stat_defs);
                        assert_eq!(
                            matches(&compile(text).unwrap(), &view),
                            expected,
                            "{text} {:?} {power:?}",
                            paired.map(|d| d.slot)
                        );
                    }
                    let weapon_names = paired
                        .map(|d| query::inventory_plug_names(d, &selected).0)
                        .unwrap_or_default();
                    for (text, expected) in [
                        ("perk:dragon", weapon_names.iter().any(|n| n == "Dragonfly")),
                        (
                            "perkname:dragon",
                            weapon_names.iter().any(|n| n == "Dragonfly"),
                        ),
                        (
                            "perk:ramp",
                            include_available
                                && paired.is_some_and(|d| d.slot == Some(ItemSlot::Kinetic)),
                        ),
                        ("exactperk:dragon", false),
                        ("mod:siphon", false),
                        ("archetype:specialist", false),
                        (
                            "mod:targeting",
                            include_available
                                && paired.is_some_and(|d| d.slot == Some(ItemSlot::Helmet)),
                        ),
                        (
                            "is:weapon",
                            paired.is_some_and(|d| d.slot == Some(ItemSlot::Kinetic)),
                        ),
                        (
                            "is:armor",
                            paired.is_some_and(|d| d.slot == Some(ItemSlot::Helmet)),
                        ),
                        ("is:locked", item.locked),
                        ("is:unlocked", !item.locked),
                        (
                            "is:inpostmaster",
                            matches!(item.location, ItemLocation::Postmaster(_)),
                        ),
                        ("exactname:\"sun shot\"", paired.is_some()),
                        ("character:unknown", false),
                    ] {
                        assert_eq!(matches(&compile(text).unwrap(), &view), expected, "{text}");
                    }
                    for text in [
                        "name:sun name:shot",
                        "name:sun name:moon",
                        "hash:1 hash:2",
                        "is:invault is:ininventory",
                        "name:sun or is:equipped",
                        "name:sun -power:>2000",
                        "perk:dragon",
                        "power:>=0",
                        "power:>65535",
                        "stat:mobility:>4294967295",
                        "stat:mobility:>30 stat:mobility:<30",
                    ] {
                        let typed = compile(text).unwrap();
                        let prefilter = match lower(&typed) {
                            Lowering::Full(q) => q,
                            Lowering::Residual { prefilter, .. } => prefilter,
                        };
                        if matches(&typed, &view) {
                            assert!(
                                query::instance_matches_with_definition(
                                    &prefilter, &item, definition
                                ) && query::instance_matches_stats(&prefilter, &item, &stat_defs),
                                "unsafe prefilter: {text}"
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn boolean_missing_data_and_referenced_stats() {
    let def = definition();
    let mut item = instance();
    let stats = BTreeMap::from([("mobility".into(), vec![30])]);
    let characters = BTreeMap::new();
    for power in [None, Some(0), Some(2001)] {
        item.power = power;
        let view = ItemView {
            instance: &item,
            definition: Some(&def),
            socketed: &[],
            available: &[],
            include_available: false,
            stats: &stats,
            characters: &characters,
        };
        for (text, expected) in [
            ("", true),
            ("name:sun", true),
            ("exactname:\"sun shot\"", true),
            ("sun or moon is:invault", true),
            ("sun or moon is:equipped", false),
            ("-power:>2000", power != Some(2001)),
            ("power:<=0", false),
            ("-stat:resilience:>0", true),
            ("stat:mobility:>=30", true),
        ] {
            assert_eq!(
                matches(&compile(text).unwrap(), &view),
                expected,
                "{text} {power:?}"
            );
        }
    }
    assert_eq!(
        referenced_stats(
            &compile("stat:mobility:>1 or -stat:mobility:>2 stat:resilience:0").unwrap()
        ),
        vec!["mobility", "resilience"]
    );
}
#[test]
fn comparisons_and_lowering_bounds() {
    let def = definition();
    let mut item = instance();
    let characters = BTreeMap::new();
    for value in [-1, 0, 29, 30, 31, i32::MAX] {
        let stats = BTreeMap::from([("mobility".into(), vec![value])]);
        for power in [None, Some(0), Some(29), Some(30), Some(31), Some(u16::MAX)] {
            item.power = power;
            let view = ItemView {
                instance: &item,
                definition: Some(&def),
                socketed: &[],
                available: &[],
                include_available: false,
                stats: &stats,
                characters: &characters,
            };
            for (op, stat_expected, power_expected) in [
                (">=", value >= 30, power.is_some_and(|v| v >= 30)),
                (">", value > 30, power.is_some_and(|v| v > 30)),
                ("<=", value <= 30, power.is_some_and(|v| v != 0 && v <= 30)),
                ("<", value < 30, power.is_some_and(|v| v != 0 && v < 30)),
                ("=", value == 30, power == Some(30)),
            ] {
                for (text, expected) in [
                    (format!("stat:mobility:{op}30"), stat_expected),
                    (format!("power:{op}30"), power_expected),
                ] {
                    let typed = compile(&text).unwrap();
                    assert_eq!(matches(&typed, &view), expected, "{text}");
                    assert_eq!(
                        matches(&compile(&format!("-{text}")).unwrap(), &view),
                        !expected
                    );
                }
            }
        }
    }
    for (text, retained) in [
        (
            std::iter::repeat_n("stat:mobility:>0", 9)
                .collect::<Vec<_>>()
                .join(" "),
            8,
        ),
        (
            std::iter::repeat_n("tag:favorite", 17)
                .collect::<Vec<_>>()
                .join(" "),
            16,
        ),
    ] {
        let Lowering::Residual { prefilter, .. } = lower(&compile(&text).unwrap()) else {
            panic!("structured bounds must not be exceeded");
        };
        assert!(prefilter.validate().is_ok());
        let terms = prefilter
            .definitions
            .map_or(prefilter.stats.len(), |d| d.tags.len());
        assert_eq!(terms, retained);
    }
    let Lowering::Full(empty) = lower(&compile("").unwrap()) else {
        panic!("empty lowers fully");
    };
    assert_eq!(empty, base_query());
}
#[test]
fn lowering_full_and_safe_residual() {
    let def = definition();
    let item = instance();
    let stats = BTreeMap::new();
    let characters = BTreeMap::new();
    let view = ItemView {
        instance: &item,
        definition: Some(&def),
        socketed: &[],
        available: &[],
        include_available: false,
        stats: &stats,
        characters: &characters,
    };
    let typed = compile("name:sun type:\"hand cannon\" tag:favorite is:invault hash:1").unwrap();
    let Lowering::Full(query) = lower(&typed) else {
        panic!("lossless conjunction");
    };
    let mut expected = base_query();
    let mut definitions = base_definitions();
    definitions.text = Some(term("sun"));
    definitions.item_type = Some(term("hand cannon"));
    definitions.tags.push(term("favorite"));
    expected.definitions = Some(Box::new(definitions));
    expected.location = Some(LocationFilter::Vault);
    expected.item_hashes = ItemHashFilter::AnyOf(vec![ItemHash::new(1)]);
    assert_eq!(query, expected);
    assert!(destiny_core::query::instance_matches_with_definition(
        &query,
        &item,
        Some(&def)
    ));
    for text in [
        "name:sun name:shot",
        "name:sun or name:moon",
        "-is:equipped",
        "perk:sun",
        "power:>=0",
    ] {
        let typed = compile(text).unwrap();
        let Lowering::Residual {
            prefilter,
            predicate,
        } = lower(&typed)
        else {
            panic!("{text}");
        };
        assert_eq!(predicate, typed);
        if matches(&typed, &view) {
            assert!(
                destiny_core::query::instance_matches_with_definition(
                    &prefilter,
                    &item,
                    Some(&def)
                ),
                "{text}"
            );
        }
    }
}
