use destiny_core::StatOp;
use destiny_core::domain::{ClassType, DamageType, ItemSlot};
use destiny_core::search_query::{
    Comparison, FilterFormat, Leaf, QueryParseErrorKind as Error, TypedQueryKind as Kind, compile,
    list_filters,
};

fn leaf(input: &str) -> Leaf {
    let query = compile(input).unwrap();
    let Kind::Leaf(leaf) = query.kind else {
        panic!("expected leaf: {input}");
    };
    leaf
}

#[test]
fn simple_values_have_the_expected_types() {
    use Leaf::*;
    for (value, expected) in [
        ("locked", Locked),
        ("unlocked", Unlocked),
        ("equipped", Equipped),
        ("invault", Vault),
        ("ininventory", Inventory),
        ("inpostmaster", Postmaster),
        ("weapon", Weapon),
        ("armor", Armor),
        ("kinetic", Element(DamageType::Kinetic)),
        ("arc", Element(DamageType::Arc)),
        ("solar", Element(DamageType::Solar)),
        ("void", Element(DamageType::Void)),
        ("stasis", Element(DamageType::Stasis)),
        ("strand", Element(DamageType::Strand)),
        ("kineticslot", Slot(ItemSlot::Kinetic)),
        ("energy", Slot(ItemSlot::Energy)),
        ("power", Slot(ItemSlot::Power)),
        ("helmet", Slot(ItemSlot::Helmet)),
        ("gauntlets", Slot(ItemSlot::Gauntlets)),
        ("chest", Slot(ItemSlot::Chest)),
        ("leg", Slot(ItemSlot::Legs)),
        ("classitem", Slot(ItemSlot::ClassItem)),
        ("exotic", Tier("exotic")),
        ("legendary", Tier("legendary")),
        ("rare", Tier("rare")),
        ("uncommon", Tier("uncommon")),
        ("common", Tier("common")),
        ("hunter", Class(ClassType::Hunter)),
        ("titan", Class(ClassType::Titan)),
        ("warlock", Class(ClassType::Warlock)),
    ] {
        assert_eq!(leaf(&format!("IS:{}", value.to_uppercase())), expected);
        for input in [format!("not:{value}"), format!("-is:{value}")] {
            let Kind::Not(child) = compile(&input).unwrap().kind else {
                panic!("expected negation");
            };
            assert_eq!(child.kind, Kind::Leaf(expected.clone()));
        }
    }
}

#[test]
fn text_filters_normalize_and_preserve_distinct_semantics() {
    for (key, constructor) in [
        ("name", Leaf::Name as fn(String) -> Leaf),
        ("exactname", Leaf::ExactName),
        ("type", Leaf::Type),
        ("tag", Leaf::Tag),
        ("perk", Leaf::Perk),
        ("perkname", Leaf::PerkName),
        ("exactperk", Leaf::ExactPerk),
        ("mod", Leaf::Mod),
        ("archetype", Leaf::Archetype),
        ("character", Leaf::Character),
        ("keyword", Leaf::Keyword),
    ] {
        assert_eq!(
            leaf(&format!("{key}:\"  ÉCHO   Song \"")),
            constructor("écho song".into())
        );
    }
    assert_eq!(leaf("\"ÉCHO Song\""), Leaf::Keyword("écho song".into()));
    assert_eq!(leaf("hash:4294967295"), Leaf::Hash(u32::MAX));
    assert_eq!(leaf("perkhash:0"), Leaf::PerkHash(0));
}

#[test]
fn comparators_aliases_and_unsigned_boundaries() {
    for (operator, op) in [
        ("", StatOp::Equals),
        ("=", StatOp::Equals),
        ("<", StatOp::LessThan),
        ("<=", StatOp::AtMost),
        (">", StatOp::GreaterThan),
        (">=", StatOp::AtLeast),
    ] {
        for n in [0, 42, u32::MAX] {
            let cmp = Comparison { op, value: n };
            assert_eq!(leaf(&format!("power:{operator}{n}")), Leaf::Power(cmp));
            assert_eq!(leaf(&format!("light:{operator}{n}")), Leaf::Power(cmp));
            assert_eq!(
                leaf(&format!("stat:MoBiLiTy:{operator}{n}")),
                Leaf::Stat {
                    name: "mobility".into(),
                    cmp,
                }
            );
        }
    }
    assert_eq!(
        leaf("stat:\"  New   Stat :>=3\""),
        Leaf::Stat {
            name: "new stat".into(),
            cmp: Comparison {
                op: StatOp::AtLeast,
                value: 3
            },
        }
    );
}

#[test]
fn invalid_values_are_spanned_and_sanitized_even_inside_or() {
    for input in [
        "power:abc",
        "power:4294967296",
        "power:-1",
        "power:+1",
        "power:1.5",
        "power:>>3",
        "power:==3",
        "power:>=",
        "hash:4294967296",
        "hash:>3",
        "perkhash:-1",
        "is:banana",
        "stat:mobility:>>3",
        "stat:mobility:4294967296",
        "stat:mobility:-1",
        "stat:mobility:+1",
        "stat:mobility:1.5",
        "stat:mobility",
        "stat::3",
        "stat:mobility:3:4",
        "name:\"   \"",
        "name:\"a\u{7}b\"",
    ] {
        let error = compile(input).unwrap_err();
        assert_eq!(error.kind, Error::InvalidFilterValue, "{input}");
        assert_eq!(error.span, 0..input.len());
        assert!(!error.message.contains(input));
        let query = format!("is:locked or {input}");
        let error = compile(&query).unwrap_err();
        assert_eq!(error.kind, Error::InvalidFilterValue, "{query}");
        assert_eq!(error.span, 13..query.len());
    }
    let error = compile("is:locked or mysterious:value").unwrap_err();
    assert_eq!(error.kind.as_str(), "unknown_filter");
    assert_eq!(error.span, 13..29);
}

#[test]
fn unsupported_filters_are_named_without_echoing_values() {
    for input in [
        "is:wishlist",
        "wishlist:secret",
        "notes:secret",
        "is:crafted",
        "is:patternunlocked",
        "kills:42",
        "is:inloadout",
        "inloadout:secret",
        "is:prismatic",
        "stat:mobility+recovery:3",
        "stat:mobility&recovery:3",
        "basestat:mobility:3",
        "is:dupe",
        "sort:power",
        "sortby:power",
    ] {
        let error = compile(input).unwrap_err();
        assert_eq!(error.kind.as_str(), "unsupported_filter", "{input}");
        assert_eq!(error.span, 0..input.len());
        let key = if input.starts_with("is:") {
            input
        } else {
            input.split(':').next().unwrap()
        };
        assert!(error.message.contains(key), "{input}: {error}");
        assert!(!error.message.contains("secret"));
    }
    assert_eq!(
        compile("newfilter:value").unwrap_err().kind.as_str(),
        "unknown_filter"
    );
}

#[test]
fn registry_is_sorted_stable_complete_and_examples_compile() {
    let specs = list_filters();
    assert!(std::ptr::eq(specs, list_filters()));
    assert!(
        specs
            .windows(2)
            .all(|pair| pair[0].keys[0] < pair[1].keys[0])
    );
    let mut keys = std::collections::HashSet::new();
    for spec in specs {
        assert!(!spec.description.is_empty());
        assert!(!spec.examples.is_empty());
        for key in spec.keys {
            assert!(keys.insert(*key), "duplicate {key}");
        }
        for example in spec.examples {
            if spec.format == FilterFormat::Unsupported {
                assert_eq!(compile(example).unwrap_err().kind, Error::UnsupportedFilter);
            } else {
                compile(example).unwrap();
            }
        }
    }
    for spec in specs.iter().filter(|spec| spec.keys.contains(&"power")) {
        for key in spec.keys {
            assert_eq!(
                leaf(&format!("{key}:3")),
                Leaf::Power(Comparison {
                    op: StatOp::Equals,
                    value: 3
                })
            );
        }
    }
}

#[test]
fn realistic_tree_preserves_spans_and_boolean_structure() {
    let input = "is:weapon (is:arc or is:solar) -is:equipped power:>=2000 perkname:\"Volt Shot\"";
    let query = compile(input).unwrap();
    assert_eq!(query.span, 0..input.len());
    let Kind::And(children) = query.kind else {
        panic!("expected AND");
    };
    assert_eq!(children.len(), 5);
    assert_eq!(children[0].kind, Kind::Leaf(Leaf::Weapon));
    let Kind::Or(alternatives) = &children[1].kind else {
        panic!("expected OR");
    };
    assert_eq!(
        alternatives[0].kind,
        Kind::Leaf(Leaf::Element(DamageType::Arc))
    );
    assert_eq!(
        alternatives[1].kind,
        Kind::Leaf(Leaf::Element(DamageType::Solar))
    );
    let Kind::Not(child) = &children[2].kind else {
        panic!("expected NOT");
    };
    assert_eq!(child.kind, Kind::Leaf(Leaf::Equipped));
    assert_eq!(
        children[3].kind,
        Kind::Leaf(Leaf::Power(Comparison {
            op: StatOp::AtLeast,
            value: 2000
        }))
    );
    assert_eq!(
        children[4].kind,
        Kind::Leaf(Leaf::PerkName("volt shot".into()))
    );
    assert_eq!(compile(" /* empty */ ").unwrap().kind, Kind::Noop);
}
