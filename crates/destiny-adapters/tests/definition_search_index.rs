use destiny_adapters::sqlite::SqliteStore;
use destiny_core::domain::{ClassType, DamageType, ItemSlot};
use destiny_core::{
    DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery, DefinitionVersion,
    ItemDefinition, ItemHash, ManifestDefinitions, SearchRepository, SearchTerm, SourceMarker,
    Timestamp, definition_matches,
};

fn catalog() -> Vec<ItemDefinition> {
    [
        "  ÉLAN   Vital! ",
        "élan vital!",
        "İSTANBUL_100%",
        "Other",
        "Empty Mod Socket",
        "Rampage",
        "Armorer",
        "Harmonic Siphon",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, name)| ItemDefinition {
        hash: ItemHash::new(i as i64 + 1),
        bucket_hash: None,
        name: name.into(),
        item_type: (i % 2 == 0).then(|| " Auto   RIFLE ".into()),
        tier: (i % 2 == 0).then(|| " Legendary ".into()),
        element: (i % 2 == 0).then(|| "arc".into()),
        slot: (i % 2 == 0).then_some(ItemSlot::Energy),
        class_type: (i % 2 == 0).then_some(ClassType::Any),
        tags: vec![
            " PvE ".into(),
            if i % 2 == 0 { "Élite" } else { "élite extra" }.into(),
            "x\ny".into(),
        ],
        plug_category: match i {
            0 | 1 | 5 => Some("frames".into()),
            4 | 7 => Some("enhancements.v2_head".into()),
            6 => Some("armor_archetypes".into()),
            _ => None,
        },
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        version: DefinitionVersion::new("v1"),
    })
    .collect()
}
fn publish(store: &mut SqliteStore) {
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: catalog(),
            },
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        })
        .unwrap();
}
fn assert_query(store: &SqliteStore, q: &DefinitionSearchQuery) {
    let expected: Vec<_> = catalog()
        .into_iter()
        .filter(|d| definition_matches(q, d))
        .take(q.limit)
        .collect();
    assert_eq!(store.search_definitions(q).unwrap(), expected, "{q:?}");
}
fn assert_parity(store: &SqliteStore) {
    let definitions = catalog();
    for text in [
        None,
        Some("ÉLAN"),
        Some("elan"),
        Some("100%"),
        Some("missing"),
    ] {
        for mask in 0..64 {
            for limit in [1, 3, 100] {
                let q = DefinitionSearchQuery {
                    text: text.map(|s| SearchTerm::new(s).unwrap()),
                    item_type: (mask & 1 != 0).then(|| SearchTerm::new("auto rifle").unwrap()),
                    tier: (mask & 2 != 0).then(|| SearchTerm::new("legendary").unwrap()),
                    tags: if mask & 4 != 0 {
                        vec![
                            SearchTerm::new("pve").unwrap(),
                            SearchTerm::new("élite").unwrap(),
                        ]
                    } else {
                        vec![]
                    },
                    element: (mask & 8 != 0).then_some(DamageType::Arc),
                    slot: (mask & 16 != 0).then_some(ItemSlot::Energy),
                    class_type: (mask & 32 != 0).then_some(ClassType::Any),
                    limit,
                };
                assert_query(store, &q);
            }
        }
    }
    let base = DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 100,
    };
    for tag in ["x y", "x", "y", "élite extra", "missing"] {
        assert_query(
            store,
            &DefinitionSearchQuery {
                tags: vec![SearchTerm::new(tag).unwrap()],
                ..base.clone()
            },
        );
    }
    for q in [
        DefinitionSearchQuery {
            element: Some(DamageType::Solar),
            ..base.clone()
        },
        DefinitionSearchQuery {
            slot: Some(ItemSlot::Helmet),
            ..base.clone()
        },
        DefinitionSearchQuery {
            class_type: Some(ClassType::Titan),
            ..base.clone()
        },
        DefinitionSearchQuery {
            item_type: Some(SearchTerm::new("auto").unwrap()),
            ..base.clone()
        },
        DefinitionSearchQuery {
            tier: Some(SearchTerm::new("exotic").unwrap()),
            ..base.clone()
        },
    ] {
        assert_query(store, &q);
    }
    let mut tier = SearchTerm::new("legend").unwrap();
    tier.resolve(
        &destiny_core::name_matching::NameCatalog::new(["Legendary", "Legendary Plus"]),
        "mod",
    )
    .unwrap();
    assert_query(
        store,
        &DefinitionSearchQuery {
            tier: Some(tier),
            ..base
        },
    );
    for kind in ["perk", "mod", "archetype", "unknown"] {
        let expected = destiny_core::name_matching::NameCatalog::new(
            definitions
                .iter()
                .filter(|d| destiny_core::name_matching::is_name_kind(d, kind))
                .map(|d| d.name.as_str()),
        );
        let actual = store.name_catalog(kind).unwrap();
        for input in ["élan", "ramp", "empty", "armorer", "harmonic", "missing"] {
            let display = |result: Result<Vec<String>, destiny_core::AppError>| {
                result.map_err(|e| (e.category(), e.to_string()))
            };
            assert_eq!(
                display(actual.resolve(kind, input)),
                display(expected.resolve(kind, input))
            );
        }
    }
}
#[test]
fn definition_search_index_publication_and_name_parity() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    publish(&mut store);
    assert_parity(&store);
    assert!(
        store.schema_version().unwrap() >= 22,
        "compact search projection migration required"
    );
}
#[test]
fn definition_search_index_legacy_catalog_backfill() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(21).unwrap();
    publish(&mut store);
    store.migrate_to(22).unwrap();
    assert_parity(&store);
    store.migrate_to(21).unwrap();
    store.migrate_to(22).unwrap();
    assert_parity(&store);
}
