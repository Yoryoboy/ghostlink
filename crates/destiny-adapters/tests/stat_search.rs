//! Stat search parity and named output through the real application service.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{InMemorySearchRepository, QueuedProfileGateway};
use destiny_app::{ProfileSyncService, QueryService, SearchInventoryRequest};
use destiny_core::*;
use destiny_core::{domain::ItemSlot, ports::WorldDefinitionRepository};

fn definitions(
    stats: Vec<StatDefinition>,
) -> (DefinitionReplacementInput, WorldDefinitionsAcquisition) {
    let version = DefinitionVersion::new("v1");
    let items = DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: version.clone(),
            definitions: vec![ItemDefinition {
                hash: ItemHash::new(1),
                bucket_hash: None,
                name: "Helmet".into(),
                item_type: None,
                tier: Some("Exotic".into()),
                element: None,
                slot: Some(ItemSlot::Helmet),
                class_type: None,
                plug_category: None,
                tags: vec![],
                version: version.clone(),
            }],
        },
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    };
    let world = WorldDefinitionsAcquisition {
        version,
        stats: WorldDefinitionTable {
            definitions: stats,
            skipped_count: 0,
        },
        milestones: WorldDefinitionTable {
            definitions: vec![
                MilestoneDefinition::new(1, "Milestone".into(), "".into(), 1).unwrap(),
            ],
            skipped_count: 0,
        },
        activities: WorldDefinitionTable {
            definitions: vec![
                ActivityDefinition::new(1, "Activity".into(), "".into(), 1, 1).unwrap(),
            ],
            skipped_count: 0,
        },
        modifiers: WorldDefinitionTable {
            definitions: vec![
                ActivityModifierDefinition::new(1, "Modifier".into(), "".into()).unwrap(),
            ],
            skipped_count: 0,
        },
        objectives: WorldDefinitionTable {
            definitions: vec![ObjectiveDefinition::new(1, "".into(), 1).unwrap()],
            skipped_count: 0,
        },
        activity_types: WorldDefinitionTable {
            definitions: vec![ActivityTypeDefinition::new(1, "Type".into(), "".into()).unwrap()],
            skipped_count: 0,
        },
    };
    (items, world)
}
fn query(conditions: &[&str]) -> InstanceSearchQuery {
    InstanceSearchQuery {
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
        stats: conditions.iter().map(|s| s.parse().unwrap()).collect(),
        limit: 1,
    }
}
fn seeded(stats: Vec<StatDefinition>) -> (SqliteStore, InMemorySearchRepository) {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let (items, world) = definitions(stats.clone());
    let mut fake =
        InMemorySearchRepository::with_definitions(items.definitions.definitions.clone());
    fake.set_stat_definitions(stats.clone());
    if stats.is_empty() {
        store.replace_definitions(items).unwrap();
    } else {
        store.publish_manifest(items, world).unwrap();
    }
    let instances = [
        ("a", vec![]),
        (
            "b",
            vec![
                StatSummary {
                    stat_hash: 1,
                    value: 5,
                },
                StatSummary {
                    stat_hash: 2,
                    value: 35,
                },
                StatSummary {
                    stat_hash: 3,
                    value: 20,
                },
                StatSummary {
                    stat_hash: 99,
                    value: 90,
                },
            ],
        ),
    ]
    .into_iter()
    .map(|(id, stats)| ItemInstance {
        available_plugs: Vec::new(),
        instance_id: id.into(),
        item_hash: ItemHash::new(1),
        location: if id == "b" {
            ItemLocation::Character(CharacterId::new("123"))
        } else {
            ItemLocation::Vault
        },
        quantity: 1,
        equipped: id == "b",
        locked: false,
        power: None,
        perks: vec![],
        plugs: vec![],
        stats,
    })
    .collect::<Vec<_>>();
    fake.set_instances(instances.clone());
    store
        .replace_current(InventorySnapshotInput {
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: DefinitionVersion::new("v1"),
            checked_at: Timestamp::new(1),
            source: SourceMarker::Profile,
            characters: vec![Character {
                id: CharacterId::new("123"),
                name: None,
                class: None,
                race: None,
                gender: None,
                light: None,
            }],
            items: instances,
        })
        .unwrap();
    (store, fake)
}
fn catalog() -> Vec<StatDefinition> {
    vec![
        StatDefinition::new(1, "Class".into()).unwrap(),
        StatDefinition::new(2, "Class".into()).unwrap(),
        StatDefinition::new(3, "Health".into()).unwrap(),
    ]
}
#[test]
fn tier_location_equipped_search_has_sqlite_fake_parity() {
    let (store, fake) = seeded(vec![]);
    for (location, equipped, character, tier, count) in [
        (Some(LocationFilter::Vault), Some(false), None, None, 1),
        (Some(LocationFilter::Character), None, None, None, 1),
        (None, Some(true), None, None, 1),
        (
            Some(LocationFilter::Character),
            Some(true),
            Some("123"),
            Some("EXOTIC"),
            1,
        ),
        (Some(LocationFilter::Character), Some(false), None, None, 0),
        (Some(LocationFilter::Vault), None, Some("123"), None, 0),
        (None, None, None, Some("exotic"), 1),
        (None, None, None, Some("legendary"), 0),
        (None, None, None, Some("imaginary"), 0),
    ] {
        let mut q = query(&[]);
        q.location = location;
        q.equipped = equipped;
        q.character_id = character.map(CharacterId::new);
        if let Some(tier) = tier {
            q.definitions = Some(Box::new(DefinitionSearchQuery {
                tier: Some(SearchTerm::new(tier).unwrap()),
                text: None,
                item_type: None,
                tags: vec![],
                element: None,
                slot: None,
                class_type: None,
                limit: 1,
            }));
        }
        let actual = store.search_instances(&q).unwrap();
        assert_eq!(actual, fake.search_instances(&q).unwrap());
        assert_eq!(actual.len(), count);
        if count > 0 && (equipped == Some(true) || location == Some(LocationFilter::Character)) {
            assert_eq!(actual[0].instance_id.as_str(), "b");
        }
        if let Some(definitions) = &q.definitions {
            let actual = store.search_definitions(definitions).unwrap();
            assert_eq!(actual, fake.search_definitions(definitions).unwrap());
            assert_eq!(
                actual.len(),
                usize::from(tier.is_some_and(|tier| tier.eq_ignore_ascii_case("exotic")))
            );
        }
    }
}

#[test]
fn stat_search_matches_any_duplicate_name_before_limit_with_fake_parity() {
    let (store, fake) = seeded(catalog());
    for (conditions, count) in [
        (vec!["Class>=30"], 1),
        (vec!["Class<=5", "Health=20"], 1),
        (vec!["Class=35", "Class=5"], 1),
        (vec!["Class>=36"], 0),
        (vec!["Class>34", "Health<21"], 1),
        (vec!["Class>35"], 0),
        (vec!["Health<20"], 0),
        (vec![" Class >= 35 ", "HEALTH <= 20"], 1),
        (vec!["Class>=30", "Health<=19"], 0),
        (vec!["Missing=90"], 0),
    ] {
        let q = query(&conditions);
        let actual = store.search_instances(&q).unwrap();
        assert_eq!(actual, fake.search_instances(&q).unwrap());
        assert_eq!(actual.len(), count);
        if count > 0 {
            assert_eq!(actual[0].instance_id.as_str(), "b");
        }
    }
}
#[test]
fn stat_names_are_validated_before_sync_and_named_hits_are_ordered() {
    let (store, _) = seeded(catalog());
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let mut request = SearchInventoryRequest {
        definitions: DefinitionSearchQuery {
            tier: None,
            text: None,
            item_type: None,
            tags: vec![],
            element: None,
            slot: None,
            class_type: None,
            limit: 1,
        },
        instances: Some(query(&["Class>=30"])),
    };
    let result = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(
        result.hits[0].stats,
        vec![
            NamedStat {
                name: "Class".into(),
                value: 5
            },
            NamedStat {
                name: "Class".into(),
                value: 35
            },
            NamedStat {
                name: "Health".into(),
                value: 20
            }
        ]
    );
    let cli = destiny_cli::output::render_search(destiny_cli::output::OutputMode::Human, &result);
    assert!(cli.contains("stats: Class 5, Class 35, Health 20"));
    let mcp = destiny_mcp::service::search_inventory_output(&result);
    assert_eq!(mcp.hits[0].stats[1].value, 35);
    let cli_json: serde_json::Value = serde_json::from_str(&destiny_cli::output::render_search(
        destiny_cli::output::OutputMode::Json,
        &result,
    ))
    .unwrap();
    let mcp_json = serde_json::to_value(mcp).unwrap();
    assert_eq!(cli_json["hits"][0]["stats"], mcp_json["hits"][0]["stats"]);
    assert_eq!(cli_json["resolved"], mcp_json["resolved"]);
    assert_eq!(
        cli_json["resolved"]["stats"]["class"],
        serde_json::json!(["Class"])
    );
    request.instances = Some(query(&["cla>=30"]));
    let partial = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(partial.hits, result.hits);
    assert_eq!(partial.resolved.stats["cla"], ["Class"]);
    // Named stats are also shown without a stat criterion.
    request.instances = Some(query(&[]));
    request.instances.as_mut().unwrap().limit = 2;
    let unfiltered = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert!(unfiltered.hits[0].stats.is_empty());
    assert_eq!(unfiltered.hits[1].stats, result.hits[0].stats);
    request.instances = Some(query(&["secret-sentinel=1"]));
    let error = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 0 },
            Timestamp::new(100),
        )
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::UnknownStatName);
    assert!(
        error
            .to_string()
            .contains("unknown stat name \"secret-sentinel\"; did you mean:")
    );
    let cli_error = destiny_cli::command::CliError::from(error.clone());
    let mcp_error = destiny_mcp::schema::McpError::from(error.clone());
    assert_eq!(cli_error.code(), "unknown_stat_name");
    assert_eq!(cli_error.to_string(), error.to_string());
    let mcp_error = serde_json::to_value(mcp_error).unwrap();
    assert_eq!(mcp_error["code"], "unknown_stat_name");
    assert_eq!(mcp_error["message"], error.to_string());
    let (store, _) = seeded(vec![]);
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    assert_eq!(
        app.search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 0 },
            Timestamp::new(100)
        )
        .unwrap_err()
        .category(),
        ErrorCategory::DefinitionsUnavailable
    );
}
#[test]
fn stat_condition_parser_rejects_malformed_and_bounds() {
    for input in [
        "",
        ">=3",
        "Weapons",
        "Weapons>>3",
        "Class< =35",
        "Weapons==3",
        "Weapons!=3",
        "Weapons>=2147483648",
        "Weapons=3 extra",
        "Weapons\u{7}=1",
    ] {
        assert!(input.parse::<StatCondition>().is_err(), "{input:?}");
    }
    assert!(
        format!("{}=1", "a".repeat(257))
            .parse::<StatCondition>()
            .is_err()
    );
    let parsed: StatCondition = "  WEAPONS  >= -20 ".parse().unwrap();
    assert_eq!(parsed.name.as_str(), "weapons");
    assert_eq!(parsed.op, StatOp::AtLeast);
    assert_eq!(parsed.value, -20);
    assert!(query(&["Class=1"; 9]).validate().is_err());
}
