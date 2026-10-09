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
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
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
        item_descriptions: Vec::new(),
        loadout_names: WorldDefinitionTable {
            definitions: vec![],
            skipped_count: 0,
        },
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
    let mut items = items;
    for (hash, name, category) in [
        (60, "Harmonic Siphon", "enhancements.v2_head"),
        (61, "Heavy Ammo Finder", "enhancements.v2_head"),
        (62, "Specialist", "armor_archetypes"),
    ] {
        let mut plug = items.definitions.definitions[0].clone();
        plug.hash = ItemHash::new(hash);
        plug.name = name.into();
        plug.slot = None;
        plug.plug_category = Some(category.into());
        items.definitions.definitions.push(plug);
    }
    (items, world)
}
fn query(conditions: &[&str]) -> InstanceSearchQuery {
    InstanceSearchQuery {
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
        available_plugs: vec![
            AvailablePlug {
                socket_index: 0,
                plug_hash: ItemHash::new(60),
            },
            AvailablePlug {
                socket_index: 0,
                plug_hash: ItemHash::new(61),
            },
            AvailablePlug {
                socket_index: 0,
                plug_hash: ItemHash::new(99),
            },
        ],
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
        plugs: [60, 62, 99]
            .into_iter()
            .map(|hash| PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect(),
        stats,
    })
    .collect::<Vec<_>>();
    fake.set_instances(instances.clone());
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
            characters: vec![Character {
                id: CharacterId::new("123"),
                name: None,
                class: None,
                race: None,
                gender: None,
                light: None,
                date_last_played: None,
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
fn batched_projection_and_top_k_match_stable_reference() {
    let (mut store, _) = seeded(catalog());
    let snapshot = store.load_current().unwrap().unwrap();
    let mut items = Vec::new();
    for index in (0..620).rev() {
        let mut item = snapshot.items[index % 2].clone();
        item.instance_id = format!("item-{index:04}").into();
        for stat in &mut item.stats {
            stat.value = (index % 7) as i32;
        }
        items.push(item);
    }
    let expected_items = {
        let mut sorted = items.clone();
        sorted.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        sorted
    };
    let committed = store
        .replace_current(InventorySnapshotInput {
            profile_currencies_coverage: snapshot.profile_currencies_coverage,
            stacks: snapshot.stacks,
            plug_set_entries: snapshot.plug_set_entries,
            membership_id: snapshot.metadata.membership_id,
            membership_type: snapshot.metadata.membership_type,
            definition_version: snapshot.metadata.definition_version,
            checked_at: snapshot.metadata.checked_at,
            source: snapshot.metadata.source,
            characters: snapshot.characters,
            items,
        })
        .unwrap();
    assert_eq!(committed.items, expected_items);
    assert_eq!(store.load_current().unwrap(), Some(committed.clone()));
    let (definitions, _) = definitions(catalog());
    let mut fake =
        InMemorySearchRepository::with_projection(definitions.definitions.definitions, &committed);
    fake.set_stat_definitions(catalog());
    for conditions in [
        vec![],
        vec!["Class>=3"],
        vec!["Health<=2"],
        vec!["Missing=1"],
    ] {
        for sort in [
            None,
            Some("Class"),
            Some("Class:asc"),
            Some("Health"),
            Some("Health:asc"),
        ] {
            let mut paged = query(&conditions);
            paged.sort = sort.map(|s| s.parse().unwrap());
            paged.limit = 1000;
            let full = fake.search_instances(&paged).unwrap();
            paged.limit = 31;
            let mut collected = Vec::new();
            loop {
                let (page, more) = store.search_instance_page(&paged).unwrap();
                assert_eq!(page, fake.search_instances(&paged).unwrap());
                collected.extend(page);
                if !more {
                    break;
                }
                paged.offset += paged.limit;
            }
            assert_eq!(collected, full, "pagination with ties: {sort:?}");
            for limit in [1, 5, 31, 620, 1000] {
                let mut q = query(&conditions);
                q.sort = sort.map(|s| s.parse().unwrap());
                q.limit = limit;
                assert_eq!(
                    store.search_instances(&q).unwrap(),
                    fake.search_instances(&q).unwrap(),
                    "{q:?}"
                );
            }
        }
    }
}

#[test]
fn offset_pages_preserve_order_and_reject_invalid_offset() {
    let (store, fake) = seeded(catalog());
    for sort in [None, Some("Class"), Some("Class:asc")] {
        let mut q = query(&[]);
        q.sort = sort.map(|s| s.parse().unwrap());
        q.limit = 1000;
        let full = store.search_instances(&q).unwrap();
        q.limit = 1;
        let mut pages = Vec::new();
        for offset in 0..full.len() {
            q.offset = offset;
            let (page, more) = store.search_instance_page(&q).unwrap();
            assert_eq!(page, fake.search_instances(&q).unwrap());
            assert_eq!(more, offset + 1 < full.len());
            pages.extend(page);
        }
        assert_eq!(pages, full);
        q.offset = 10_000;
        assert!(store.search_instances(&q).unwrap().is_empty());
        assert!(fake.search_instances(&q).unwrap().is_empty());
        q.offset = 10_001;
        assert!(store.search_instances(&q).is_err());
    }
}

#[test]
fn owned_continuations_stop_at_the_offset_ceiling() {
    let (mut store, mut fake) = seeded(vec![]);
    let template = fake.search_instances(&query(&[])).unwrap().remove(0);
    let items: Vec<_> = (0..10_003)
        .map(|index| {
            let mut item = template.clone();
            item.instance_id = format!("item-{index:05}").into();
            item.plugs.clear();
            item.available_plugs.clear();
            item
        })
        .collect();
    fake.set_instances(items.clone());
    store
        .replace_current(InventorySnapshotInput {
            profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
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

    // Exercise the default trait fallback, not either adapter's override.
    struct Fallback(InMemorySearchRepository);
    impl SearchRepository for Fallback {
        fn search_definitions(
            &self,
            q: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.0.search_definitions(q)
        }
        fn search_instances(&self, q: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
            q.validate()?;
            assert!(
                q.offset <= 10_000,
                "fallback must not issue an invalid probe"
            );
            self.0.search_instances(q)
        }
        fn search_instance_query_page(
            &self,
            q: &InstanceSearchQuery,
            execution: Option<&search_query::ResolvedQuery>,
        ) -> Result<(Vec<ItemInstance>, bool), AppError> {
            if execution.is_none() {
                return self.search_instance_page(q);
            }
            self.0.search_instance_query_page(q, execution)
        }
        fn lookup_instance(
            &self,
            id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.0.lookup_instance(id)
        }
    }
    let fallback = Fallback(fake.clone());
    let mut q = query(&[]);
    q.limit = 2;
    for (offset, expected_more) in [(9_998, true), (9_999, false), (10_000, false)] {
        q.offset = offset;
        let (page, more) = store.search_instance_page(&q).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(more, expected_more);
        assert_eq!(
            fallback.search_instance_page(&q).unwrap(),
            (page, expected_more)
        );
    }

    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let definitions = DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 2,
    };
    for (offset, expected_next) in [(9_998, Some(10_000)), (9_999, None), (10_000, None)] {
        q.offset = offset;
        let result = app
            .search_inventory(
                &SearchInventoryRequest::new(definitions.clone(), Some(q.clone())),
                FreshnessPolicy { ttl_seconds: 300 },
                Timestamp::new(1),
            )
            .unwrap();
        assert_eq!(result.hits.len(), 2);
        assert_eq!(result.next_offset, expected_next);
    }
}

#[test]
fn stat_sort_precedes_limit_with_sqlite_fake_parity() {
    let (store, fake) = seeded(catalog());
    for expression in ["Class", "Class:asc"] {
        let mut q = query(&[]);
        q.sort = Some(expression.parse().unwrap());
        let actual = store.search_instances(&q).unwrap();
        assert_eq!(actual, fake.search_instances(&q).unwrap());
        assert_eq!(actual[0].instance_id.as_str(), "b");
        q.limit = 1000;
        let actual = store.search_instances(&q).unwrap();
        assert_eq!(actual, fake.search_instances(&q).unwrap());
        assert_eq!(
            actual
                .iter()
                .map(|i| i.instance_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
    assert_eq!(
        store.search_instances(&query(&[])).unwrap()[0]
            .instance_id
            .as_str(),
        "a"
    );
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
        query: None,
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
    let inspection = app
        .inspect_item(
            &InspectItemRequest {
                instance_id: "b".into(),
            },
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    let Resolution::Unique(inspected) = &inspection else {
        panic!("missing inspection")
    };
    assert_eq!(inspected.stats, result.hits[0].stats);
    assert_eq!(inspected.mod_names, ["Harmonic Siphon"]);
    assert_eq!(inspected.archetype.as_deref(), Some("Specialist"));
    assert_eq!(inspected.available_mods, ["Heavy Ammo Finder"]);
    let text =
        destiny_cli::output::render_inspection(destiny_cli::output::OutputMode::Human, inspected);
    assert!(text.contains("stats: Class 5, Class 35, Health 20"));
    let inspect_json: serde_json::Value = serde_json::from_str(
        &destiny_cli::output::render_inspection(destiny_cli::output::OutputMode::Json, inspected),
    )
    .unwrap();
    let inspect_mcp =
        serde_json::to_value(destiny_mcp::service::inspect_item_output(&inspection).unwrap())
            .unwrap();
    for field in [
        "perk_names",
        "mod_names",
        "mods",
        "archetype",
        "available_perks",
        "available_mods",
        "stats",
    ] {
        assert_eq!(inspect_json[field], inspect_mcp[field], "{field}");
    }
    assert_eq!(
        inspect_json["stats"],
        serde_json::json!([{"name":"Class","value":5},{"name":"Class","value":35},{"name":"Health","value":20}])
    );
    let cli =
        destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Human, &result);
    assert!(cli.contains("stats: Class 5, Class 35, Health 20"));
    let mcp = destiny_mcp::service::search_inventory_output(&result);
    assert_eq!(mcp.hits[0].stats[1].value, 35);
    let cli_json: serde_json::Value = serde_json::from_str(
        &destiny_cli::output::render_search_detail(destiny_cli::output::OutputMode::Json, &result),
    )
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
    let mut sorted_query = query(&[]);
    sorted_query.sort = Some("cla:asc".parse().unwrap());
    request.instances = Some(sorted_query);
    let sorted = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(sorted.hits[0].instance.instance_id.as_str(), "b");
    assert_eq!(sorted.resolved.sort, Some(("Class".into(), "asc".into())));
    let text = destiny_cli::output::render_search(destiny_cli::output::OutputMode::Human, &sorted);
    assert!(text.contains("sorted by: Class (asc)"));
    let cli_json: serde_json::Value = serde_json::from_str(&destiny_cli::output::render_search(
        destiny_cli::output::OutputMode::Json,
        &sorted,
    ))
    .unwrap();
    let mcp_json =
        serde_json::to_value(destiny_mcp::service::search_inventory_output(&sorted)).unwrap();
    assert_eq!(cli_json["resolved"], mcp_json["resolved"]);
    assert_eq!(
        cli_json["resolved"]["sort"],
        serde_json::json!({"name": "Class", "order": "asc"})
    );
    request.instances.as_mut().unwrap().sort = Some("secret-sentinel".parse().unwrap());
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
fn typed_request(input: &str, instances: InstanceSearchQuery) -> SearchInventoryRequest {
    SearchInventoryRequest::new(
        DefinitionSearchQuery {
            text: None,
            item_type: None,
            tier: None,
            tags: vec![],
            element: None,
            slot: None,
            class_type: None,
            limit: instances.limit,
        },
        Some(instances),
    )
    .with_query(input)
    .unwrap()
}

#[test]
fn typed_query_app_flag_parity_and_combination() {
    let (store, _) = seeded(catalog());
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    for (input, flags) in [
        ("is:invault", {
            let mut q = query(&[]);
            q.location = Some(LocationFilter::Vault);
            q
        }),
        ("is:equipped", {
            let mut q = query(&[]);
            q.equipped = Some(true);
            q
        }),
        ("stat:class:>=30", query(&["Class>=30"])),
        ("is:armor", {
            let mut q = query(&[]);
            q.definitions = Some(Box::new(typed_request("", q.clone()).definitions));
            q.definitions.as_mut().unwrap().slot = Some(ItemSlot::Helmet);
            q
        }),
    ] {
        let typed = app
            .search_inventory(
                &typed_request(input, query(&[])),
                FreshnessPolicy { ttl_seconds: 300 },
                Timestamp::new(1),
            )
            .unwrap();
        let mut request = typed_request("", flags);
        request.query = None;
        let structured = app
            .search_inventory(
                &request,
                FreshnessPolicy { ttl_seconds: 300 },
                Timestamp::new(1),
            )
            .unwrap();
        assert_eq!(typed.hits, structured.hits, "{input}");
    }
    let mut flags = query(&[]);
    flags.equipped = Some(true);
    let result = app
        .search_inventory(
            &typed_request("is:invault or is:locked", flags),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert!(result.hits.is_empty(), "query must AND structured flags");
    let request = SearchInventoryRequest::new(typed_request("", query(&[])).definitions, None)
        .with_query("")
        .unwrap();
    let result = app
        .search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1),
        )
        .unwrap();
    assert_eq!(result.hits[0].instance.instance_id.as_str(), "a");
    assert_eq!(result.next_offset, Some(1));
    let mut request = typed_request("is:legendary", query(&[]));
    request.definitions.tier = Some(SearchTerm::new("exotic").unwrap());
    assert!(
        app.search_inventory(
            &request,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1)
        )
        .unwrap()
        .hits
        .is_empty()
    );
}

/// Search is fake; committed inventory and catalogs are the same real store.
/// This isolates candidate-scan parity while exercising app resolution/enrichment.
struct FakeAppRepository {
    store: SqliteStore,
    search: InMemorySearchRepository,
}
impl InventoryRepository for FakeAppRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        self.store.load_current()
    }
    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        self.search.set_instances(input.items.clone());
        self.store.replace_current(input)
    }
}
impl DefinitionRepository for FakeAppRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.store.current_definition_version()
    }
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        self.store.lookup_definition(hash)
    }
    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        self.store.current_metadata()
    }
    fn replace_definitions(
        &mut self,
        input: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.store.replace_definitions(input)
    }
}
impl StatDefinitionRepository for FakeAppRepository {
    fn lookup_stat(&self, hash: u32) -> Result<Option<StatDefinition>, AppError> {
        self.store.lookup_stat(hash)
    }
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
        self.store.stat_hashes_by_name(name)
    }
    fn stat_definition_count(&self) -> Result<usize, AppError> {
        self.store.stat_definition_count()
    }
}
impl SearchRepository for FakeAppRepository {
    fn name_catalog(&self, kind: &str) -> Result<name_matching::NameCatalog, AppError> {
        self.search.name_catalog(kind)
    }
    fn search_definitions(
        &self,
        q: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        self.search.search_definitions(q)
    }
    fn search_instances(&self, q: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
        self.search.search_instances(q)
    }
    fn search_instance_page(
        &self,
        q: &InstanceSearchQuery,
    ) -> Result<(Vec<ItemInstance>, bool), AppError> {
        self.search.search_instance_page(q)
    }
    fn search_instance_query_page(
        &self,
        q: &InstanceSearchQuery,
        execution: Option<&search_query::ResolvedQuery>,
    ) -> Result<(Vec<ItemInstance>, bool), AppError> {
        self.search.search_instance_query_page(q, execution)
    }
    fn lookup_instance(&self, id: &ItemInstanceId) -> Result<Resolution<ItemInstance>, AppError> {
        self.search.lookup_instance(id)
    }
}

fn large_typed_fixture() -> (SqliteStore, InMemorySearchRepository) {
    let (mut store, mut fake) = seeded(catalog());
    let snapshot = store.load_current().unwrap().unwrap();
    let mut input = InventorySnapshotInput::with_metadata(&snapshot);
    input.characters[0].class = Some("Hunter".into());
    input.items = (0..620)
        .map(|index| {
            let mut item = snapshot.items[index % 2].clone();
            item.instance_id = format!("{index:04}").into();
            item.locked = index >= 580;
            item.power = match index % 5 {
                0 => None,
                1 => Some(0),
                _ => Some(2000 + (index % 20) as u16),
            };
            if index % 3 == 0 {
                item.stats.clear();
            } else {
                item.stats = vec![
                    StatSummary {
                        stat_hash: 1,
                        value: (index % 7) as i32,
                    },
                    StatSummary {
                        stat_hash: 2,
                        value: (index % 11) as i32,
                    },
                ];
            }
            item
        })
        .collect();
    fake.set_instances(input.items.clone());
    store.replace_current(input).unwrap();
    (store, fake)
}

#[test]
fn typed_residual_reference_paging_sort_and_fake_app_parity() {
    let (store, _) = large_typed_fixture();
    let snapshot = store.load_current().unwrap().unwrap();
    let base = match store.lookup_definition(ItemHash::new(1)).unwrap() {
        Resolution::Unique(d) => d,
        _ => panic!("fixture definition"),
    };
    let socketed =
        [60, 62].map(
            |hash| match store.lookup_definition(ItemHash::new(hash)).unwrap() {
                Resolution::Unique(d) => d,
                _ => panic!("fixture plug"),
            },
        );
    let available =
        [60, 61].map(
            |hash| match store.lookup_definition(ItemHash::new(hash)).unwrap() {
                Resolution::Unique(d) => d,
                _ => panic!("fixture plug"),
            },
        );
    let characters =
        std::collections::BTreeMap::from([("hunter".to_owned(), CharacterId::new("123"))]);
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let (store, search) = large_typed_fixture();
    let mut fake_app = QueryService::new(ProfileSyncService::new(
        QueuedProfileGateway::new(),
        FakeAppRepository { store, search },
    ));
    let policy = FreshnessPolicy { ttl_seconds: 300 };
    for text in [
        "",
        "is:invault",
        "is:equipped",
        "is:helmet is:exotic",
        "stat:class:>=5",
        "is:locked or exactname:missing", // Only late matches, beyond both batch sizes.
        "not (is:equipped or is:locked)",
        "(stat:class:>=5 or -power:>2010) is:armor",
        "is:locked or (character:hunter and mod:harmonic)",
        "mod:heavy or archetype:specialist",
        "hash:1 hash:99", // Colliding positive fields must not overwrite.
    ] {
        let typed = search_query::compile(text).unwrap();
        for include_available in [false, true] {
            for sort in [None, Some("Class:asc"), Some("Class:desc")] {
                let mut flags = query(&[]);
                flags.limit = 61;
                flags.include_available = include_available;
                flags.sort = sort.map(|s| s.parse().unwrap());
                // Also test an independent structured conjunction.
                for equipped in [None, Some(true)] {
                    flags.equipped = equipped;
                    let mut reference: Vec<_> = snapshot
                        .items
                        .iter()
                        .filter(|item| {
                            let mut stats: std::collections::BTreeMap<String, Vec<i32>> =
                                std::collections::BTreeMap::new();
                            for stat in &item.stats {
                                if [1, 2].contains(&stat.stat_hash) {
                                    stats.entry("class".into()).or_default().push(stat.value);
                                }
                            }
                            query::instance_matches(&flags, item)
                                && search_query::matches(
                                    &typed,
                                    &search_query::ItemView {
                                        instance: item,
                                        definition: Some(&base),
                                        socketed: &socketed,
                                        available: &available,
                                        include_available,
                                        stats: &stats,
                                        characters: &characters,
                                    },
                                )
                        })
                        .cloned()
                        .collect();
                    reference.sort_by(|a, b| {
                        query::compare_instances(flags.sort.as_ref(), a, b, &catalog())
                    });
                    let mut observed = Vec::new();
                    let mut offset = 0;
                    loop {
                        flags.offset = offset;
                        let request = typed_request(text, flags.clone());
                        let result = app
                            .search_inventory(&request, policy, Timestamp::new(1))
                            .unwrap();
                        let fake = fake_app
                            .search_inventory(&request, policy, Timestamp::new(1))
                            .unwrap();
                        assert_eq!(result, fake, "fake app parity: {text}, {sort:?}");
                        let page: Vec<_> =
                            result.hits.iter().map(|hit| hit.instance.clone()).collect();
                        assert_eq!(
                            page,
                            reference
                                .iter()
                                .skip(offset)
                                .take(flags.limit)
                                .cloned()
                                .collect::<Vec<_>>(),
                            "{text}, {sort:?}, offset {offset}"
                        );
                        let end = offset + page.len();
                        assert_eq!(result.next_offset, (end < reference.len()).then_some(end));
                        observed.extend(page);
                        match result.next_offset {
                            Some(next) => offset = next,
                            None => break,
                        }
                    }
                    assert_eq!(observed, reference);
                }
            }
        }
    }
}

#[test]
fn typed_catalog_selectors_errors_and_compact_stats() {
    let (mut store, search) = large_typed_fixture();
    let policy = FreshnessPolicy { ttl_seconds: 300 };
    let now = Timestamp::new(1);
    // Give two characters the same class to exercise the shared ambiguity rule.
    let mut input = InventorySnapshotInput::with_metadata(&store.load_current().unwrap().unwrap());
    let mut other = input.characters[0].clone();
    other.id = CharacterId::new("456");
    input.characters.push(other);
    store.replace_current(input.clone()).unwrap();
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let (mut store, _) = large_typed_fixture();
    store.replace_current(input).unwrap();
    let mut fake = QueryService::new(ProfileSyncService::new(
        QueuedProfileGateway::new(),
        FakeAppRepository { store, search },
    ));
    for (text, category) in [
        ("character:hunter", ErrorCategory::AmbiguousMatch),
        (
            "is:locked or character:missing",
            ErrorCategory::ProviderValidation,
        ),
        ("-stat:missing:>1", ErrorCategory::UnknownStatName),
        ("stat:cla:>1", ErrorCategory::UnknownStatName), // No fuzzy expansion.
    ] {
        let request = typed_request(text, query(&[]));
        let (policy, now) = if category == ErrorCategory::UnknownStatName {
            (FreshnessPolicy { ttl_seconds: 0 }, Timestamp::new(100))
        } else {
            (policy, now)
        };
        assert_eq!(
            app.search_inventory(&request, policy, now)
                .unwrap_err()
                .category(),
            category
        );
        assert_eq!(
            fake.search_inventory(&request, policy, now)
                .unwrap_err()
                .category(),
            category
        );
    }
    let result = app
        .search_inventory(
            &typed_request("character:123 stat:class:>=0", query(&[])),
            policy,
            now,
        )
        .unwrap();
    assert_eq!(
        result.hits[0].instance.location,
        ItemLocation::Character(CharacterId::new("123"))
    );
    let output: serde_json::Value = serde_json::from_str(&destiny_cli::output::render_search(
        destiny_cli::output::OutputMode::Json,
        &result,
    ))
    .unwrap();
    let tile = &output["hits"][0];
    assert!(!tile["stats"].as_array().unwrap().is_empty());
    assert!(
        tile["stats"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["name"] == "Class")
    );
    let mut flags = query(&["Health>=20"]);
    flags.sort = Some("Class:asc".parse().unwrap());
    let result = app
        .search_inventory(&typed_request("stat:class:>=0", flags), policy, now)
        .unwrap();
    assert!(result.resolved.stats.contains_key("health"));
    assert!(result.resolved.stats.contains_key("class"));
    for text in [
        "unknown:private-sentinel",
        "is:locked or",
        "(is:locked",
        "stat:class:wat",
        "notes:private-sentinel",
    ] {
        let rich = typed_request("", query(&[])).with_query(text).unwrap_err();
        // A caller using `?` at the app boundary gets the existing category;
        // U3c can retain the original spanned compiler error instead.
        let error = (|| -> Result<InventorySearchResult, AppError> {
            let request = typed_request("", query(&[])).with_query(text)?;
            app.search_inventory(&request, policy, now)
        })()
        .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(
            error.to_string(),
            format!("invalid query: {}", rich.message)
        );
        assert!(!error.to_string().contains("private-sentinel"));
    }
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
