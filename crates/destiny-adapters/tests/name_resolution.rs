//! Catalog resolution and CLI/MCP output parity through the real service.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{InMemorySearchRepository, QueuedProfileGateway};
use destiny_app::{ProfileSyncService, QueryService, SearchInventoryRequest};
use destiny_core::*;

fn definition(
    hash: i64,
    name: &str,
    category: Option<&str>,
    slot: Option<domain::ItemSlot>,
) -> ItemDefinition {
    ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        name: name.into(),
        plug_category: category.map(str::to_owned),
        slot,
        bucket_hash: None,
        item_type: None,
        tier: None,
        element: None,
        class_type: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    }
}

#[test]
fn catalog_kind_resolution_matches_sets_and_reports_surface_parity() {
    let definitions = vec![
        definition(1, "Weapon", None, Some(domain::ItemSlot::Kinetic)),
        definition(2, "Helmet", None, Some(domain::ItemSlot::Helmet)),
        definition(10, "Rampage", Some("frames"), None),
        definition(11, "Rampage Plus", Some("frames"), None),
        definition(12, "Rampage Shader", Some("shader"), None),
        definition(20, "Harmonic Siphon", Some("enhancements.v2_head"), None),
        definition(21, "Empty Mod Socket", Some("enhancements.v2_head"), None),
        definition(22, "Targeting", Some("enhancements.v2_head"), None),
        definition(30, "Paragon", Some("armor_archetypes"), None),
        definition(31, "Paragon Shader", Some("shader"), None),
    ];
    let items = [
        ("a", 1, vec![10]),
        ("b", 1, vec![11]),
        ("c", 2, vec![20, 30]),
    ]
    .into_iter()
    .map(|(id, hash, plugs)| ItemInstance {
        available_plugs: vec![
            AvailablePlug {
                socket_index: 1,
                plug_hash: ItemHash::new(if hash == 1 { 11 } else { 20 }),
            },
            AvailablePlug {
                socket_index: 2,
                plug_hash: ItemHash::new(if hash == 1 { 11 } else { 22 }),
            },
        ],
        instance_id: id.into(),
        item_hash: ItemHash::new(hash),
        location: ItemLocation::Vault,
        quantity: 1,
        equipped: false,
        locked: false,
        power: None,
        perks: vec![],
        stats: vec![],
        plugs: plugs
            .into_iter()
            .map(|hash| PlugSummary {
                enabled: true,
                plug_hash: ItemHash::new(hash),
            })
            .collect(),
    })
    .collect::<Vec<_>>();
    let mut fake = InMemorySearchRepository::with_definitions(definitions.clone());
    fake.set_instances(items.clone());
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions,
            },
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    store
        .replace_current(InventorySnapshotInput {
            profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
            stacks: Vec::new(),
            plug_set_entries: Vec::new(),
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: DefinitionVersion::new("v1"),
            checked_at: Timestamp::new(1),
            source: SourceMarker::Profile,
            characters: vec![],
            items,
        })
        .unwrap();
    let query = InstanceSearchQuery {
        offset: 0,
        sort: None,
        location: None,
        equipped: None,
        include_available: false,
        item_hashes: ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        character_id: None,
        definitions: None,
        min_power: None,
        stats: vec![],
        limit: 10,
    };
    for (kind, input, expected, count) in [
        ("perk", "ramp", vec!["Rampage", "Rampage Plus"], 2),
        ("perk", " RAMPAGE ", vec!["Rampage"], 1),
        ("mod", "siph", vec!["Harmonic Siphon"], 1),
        ("archetype", "para", vec!["Paragon"], 1),
    ] {
        let mut q = query.clone();
        let mut term = SearchTerm::new(input).unwrap();
        assert_eq!(
            term.resolve(&store.name_catalog(kind).unwrap(), kind)
                .unwrap(),
            expected
        );
        let mut fake_term = SearchTerm::new(input).unwrap();
        assert_eq!(
            fake_term
                .resolve(&fake.name_catalog(kind).unwrap(), kind)
                .unwrap(),
            expected
        );
        match kind {
            "perk" => q.perk_name = Some(vec![term]),
            "mod" => q.mod_name = Some(vec![term]),
            _ => q.archetype = Some(term),
        }
        let actual = store.search_instances(&q).unwrap();
        assert_eq!(actual, fake.search_instances(&q).unwrap());
        assert_eq!(actual.len(), count);
    }
    // The two socketed perks are on different items: neither satisfies AND.
    let mut both = query.clone();
    both.perk_name = Some(vec![
        SearchTerm::new("Rampage").unwrap(),
        SearchTerm::new("Rampage Plus").unwrap(),
    ]);
    assert!(store.search_instances(&both).unwrap().is_empty());
    assert_eq!(
        store.search_instances(&both).unwrap(),
        fake.search_instances(&both).unwrap()
    );
    both.include_available = true;
    let hits = store.search_instances(&both).unwrap();
    assert_eq!(hits, fake.search_instances(&both).unwrap());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].instance_id.as_str(), "a");
    let mut alternatives = query.clone();
    alternatives.perk_name = Some(vec![SearchTerm::new("Rampage Plus").unwrap()]);
    assert_eq!(store.search_instances(&alternatives).unwrap().len(), 1);
    alternatives.include_available = true;
    assert_eq!(store.search_instances(&alternatives).unwrap().len(), 2);
    assert_eq!(
        store.search_instances(&alternatives).unwrap(),
        fake.search_instances(&alternatives).unwrap()
    );
    alternatives.perk_name = None;
    alternatives.mod_name = Some(vec![
        SearchTerm::new("Targeting").unwrap(),
        SearchTerm::new("Harmonic Siphon").unwrap(),
    ]);
    alternatives.include_available = false;
    assert!(store.search_instances(&alternatives).unwrap().is_empty());
    alternatives.include_available = true;
    assert_eq!(store.search_instances(&alternatives).unwrap().len(), 1);
    assert_eq!(
        store.search_instances(&alternatives).unwrap(),
        fake.search_instances(&alternatives).unwrap()
    );
    for (kind, input) in [
        ("perk", "rampge"),
        ("mod", "empty"),
        ("archetype", "paragn"),
    ] {
        let error = store
            .name_catalog(kind)
            .unwrap()
            .resolve(kind, input)
            .unwrap_err();
        assert_eq!(
            error,
            fake.name_catalog(kind)
                .unwrap()
                .resolve(kind, input)
                .unwrap_err()
        );
        assert!(!error.to_string().contains("Shader"));
        assert!(!error.to_string().contains("Empty Mod Socket"));
    }
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let request = SearchInventoryRequest {
        definitions: DefinitionSearchQuery {
            tier: None,
            text: None,
            item_type: None,
            tags: vec![],
            element: None,
            slot: None,
            class_type: None,
            limit: 10,
        },
        instances: Some(InstanceSearchQuery {
            perk_name: Some(vec![SearchTerm::new("ramp").unwrap()]),
            ..query
        }),
    };
    let result = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    let cli =
        destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Human, &result);
    assert!(cli.contains("matched perk: Rampage, Rampage Plus"));
    let cli: serde_json::Value = serde_json::from_str(&destiny_cli::output::render_search_detail(
        destiny_cli::output::OutputMode::Json,
        &result,
    ))
    .unwrap();
    let mcp = serde_json::to_value(destiny_mcp::service::search_inventory_output(&result)).unwrap();
    assert_eq!(cli["resolved"], mcp["resolved"]);
    assert_eq!(
        cli["hits"][0]["available_perks"],
        serde_json::json!(["Rampage Plus"])
    );
    assert_eq!(
        cli["hits"][0]["available_perks"],
        mcp["hits"][0]["available_perks"]
    );
    assert_eq!(cli["hits"][1]["available_perks"], serde_json::json!([]));
    let mut armor_request = request.clone();
    armor_request.instances = Some(alternatives);
    let armor = app
        .search_inventory(
            &armor_request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(armor.hits[0].available_mods, ["Targeting"]);
    assert_eq!(
        armor.resolved.mod_terms,
        vec![
            vec!["Targeting".to_owned()],
            vec!["Harmonic Siphon".to_owned()]
        ]
    );
    let armor_cli: serde_json::Value = serde_json::from_str(
        &destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Json, &armor),
    )
    .unwrap();
    let armor_mcp =
        serde_json::to_value(destiny_mcp::service::search_inventory_output(&armor)).unwrap();
    assert_eq!(
        armor_cli["hits"][0]["available_mods"],
        armor_mcp["hits"][0]["available_mods"]
    );
    assert!(
        destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Human, &armor)
            .contains("available mods: Targeting")
    );
    assert!(
        destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Human, &result)
            .contains("available perks: Rampage Plus")
    );
    assert_eq!(
        cli["resolved"]["perk"],
        serde_json::json!(["Rampage", "Rampage Plus"])
    );
    assert_eq!(
        cli["resolved"]["perk_terms"],
        serde_json::json!([["Rampage", "Rampage Plus"]])
    );
    let mut repeated = request.clone();
    repeated.instances.as_mut().unwrap().perk_name = Some(vec![
        SearchTerm::new("ramp").unwrap(),
        SearchTerm::new("Rampage").unwrap(),
    ]);
    let repeated_result = app
        .search_inventory(
            &repeated,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(repeated_result.hits.len(), 1);
    let text = destiny_cli::output::render_search(
        destiny_cli::output::OutputMode::Human,
        &repeated_result,
    );
    assert!(text.starts_with("matched perk: Rampage, Rampage Plus\nmatched perk: Rampage\n"));
    let repeated_cli: serde_json::Value =
        serde_json::from_str(&destiny_cli::output::render_search(
            destiny_cli::output::OutputMode::Json,
            &repeated_result,
        ))
        .unwrap();
    let repeated_mcp = serde_json::to_value(destiny_mcp::service::search_inventory_output(
        &repeated_result,
    ))
    .unwrap();
    assert_eq!(repeated_cli["resolved"], repeated_mcp["resolved"]);
    assert_eq!(
        repeated_cli["resolved"]["perk_terms"],
        serde_json::json!([["Rampage", "Rampage Plus"], ["Rampage"]])
    );
    repeated
        .instances
        .as_mut()
        .unwrap()
        .perk_name
        .as_mut()
        .unwrap()[1] = SearchTerm::new("rampge").unwrap();
    let error = app
        .search_inventory(
            &repeated,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::UnknownSearchName);
    assert!(error.to_string().contains("did you mean"));
}
