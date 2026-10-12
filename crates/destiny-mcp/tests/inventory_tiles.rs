//! Real CLI/MCP inventory hit parity, including legacy detail and omission rules.
use destiny_app::*;
use destiny_cli::output::{OutputMode, render_search, render_search_detail};
use destiny_mcp::service::search_inventory_output;
use serde_json::{Value, json};

fn fixture() -> InventorySearchResult {
    let definition = ItemDefinition {
        armor: Default::default(),
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(42),
        bucket_hash: None,
        name: "Test Helmet".into(),
        item_type: Some("Helmet".into()),
        tier: Some("Legendary".into()),
        element: Some("Arc".into()),
        slot: Some("helmet".parse().unwrap()),
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    };
    let mut result = InventorySearchResult {
        next_offset: None,
        resolved: Default::default(),
        candidates: vec![definition.clone()],
        hits: vec![InventorySearchHit {
            definition,
            instance: ItemInstance {
                energy: None,
                gear_tier: None,
                sockets: Vec::new(),
                transfer_status: None,
                available_plugs: vec![],
                instance_id: ItemInstanceId::new("123"),
                item_hash: ItemHash::new(42),
                location: ItemLocation::Vault,
                quantity: 1,
                equipped: false,
                locked: true,
                power: Some(2010),
                stats: vec![],
                plugs: vec![],
                perks: vec![PerkSummary {
                    perk_hash: PerkHash::new(99),
                    active: true,
                    visible: true,
                }],
            },
            perk_names: vec!["Plugged Perk".into()],
            mod_names: vec!["Plugged Mod".into()],
            available_perks: vec!["Alternative Perk".into()],
            available_mods: vec!["Alternative Mod".into()],
            archetype: Some("Heavy".into()),
            stats: vec![
                NamedStat {
                    name: "Mobility".into(),
                    value: 30,
                },
                NamedStat {
                    name: "Recovery".into(),
                    value: 20,
                },
                NamedStat {
                    name: "Resilience".into(),
                    value: 10,
                },
            ],
        }],
        metadata: QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(3),
                definition_version: DefinitionVersion::new("v1"),
            },
            freshness: FreshnessStatus::Fresh,
        },
    };
    result
        .resolved
        .stats
        .insert("mobility".into(), vec!["Mobility".into()]);
    result.resolved.sort = Some(("Recovery".into(), "desc".into()));
    result
}

#[derive(Clone)]
struct Reads(InventorySearchResult);

impl QueryOperations for Reads {
    fn postmaster(
        &mut self,
        _: Option<&CharacterId>,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<PostmasterResult, AppError> {
        unreachable!()
    }
    fn subclass_options(
        &mut self,
        _: Option<&CharacterId>,
        _: Option<SubclassElement>,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<SubclassOptionsResult, AppError> {
        unreachable!()
    }
    fn character_subclasses(
        &mut self,
        _: Option<&CharacterId>,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<SubclassResult, AppError> {
        unreachable!()
    }
    fn list_characters(
        &mut self,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<CharacterListResult, AppError> {
        unreachable!()
    }
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<InventorySearchResult, AppError> {
        let mut output = self.0.clone();
        let mut fallback =
            search_query::ResolvedQuery::new(&search_query::compile("").unwrap()).prefilter;
        fallback.limit = request.definitions.limit;
        let query = request.instances.as_ref().unwrap_or(&fallback);
        query.validate()?;
        let hits: Vec<_> = output
            .hits
            .into_iter()
            .filter(|hit| {
                request.definitions.text.as_ref().is_none_or(|term| {
                    normalize_search_text(&hit.definition.name).contains(term.as_str())
                }) && request.query.as_ref().is_none_or(|predicate| {
                    search_query::matches(
                        predicate,
                        &search_query::ItemView {
                            instance: &hit.instance,
                            definition: Some(&hit.definition),
                            socketed: &[],
                            available: &[],
                            include_available: query.include_available,
                            stats: &Default::default(),
                            characters: &Default::default(),
                        },
                    )
                })
            })
            .collect();
        output.next_offset =
            (hits.len() > query.offset + query.limit).then_some(query.offset + query.limit);
        output.hits = hits
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(output)
    }
    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
        _: FreshnessPolicy,
        _: Timestamp,
    ) -> Result<Resolution<ItemInspection>, AppError> {
        let Some(hit) = self
            .0
            .hits
            .iter()
            .find(|hit| hit.instance.instance_id == request.instance_id)
        else {
            return Ok(Resolution::Missing);
        };
        Ok(Resolution::Unique(ItemInspection {
            armor_model: None,
            definition: hit.definition.clone(),
            instance: hit.instance.clone(),
            metadata: self.0.metadata.clone(),
            perk_names: hit.perk_names.clone(),
            mod_names: hit.mod_names.clone(),
            archetype: hit.archetype.clone(),
            available_perks: hit.available_perks.clone(),
            available_mods: hit.available_mods.clone(),
            stats: hit.stats.clone(),
            plug_details: vec![],
        }))
    }
}

impl destiny_mcp::service::InventoryReadService for Reads {
    fn get_postmaster(
        &mut self,
        _: Option<&CharacterId>,
    ) -> Result<destiny_mcp::schema::GetPostmasterOutput, destiny_mcp::schema::McpError> {
        unreachable!()
    }
    fn get_subclass(
        &mut self,
        _: Option<&CharacterId>,
    ) -> Result<destiny_mcp::schema::GetSubclassOutput, destiny_mcp::schema::McpError> {
        unreachable!()
    }
    fn list_characters(
        &mut self,
        _: &destiny_mcp::service::ListCharactersRequest,
    ) -> Result<destiny_mcp::schema::ListCharactersOutput, destiny_mcp::schema::McpError> {
        unreachable!()
    }
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<destiny_mcp::schema::SearchInventoryOutput, destiny_mcp::schema::McpError> {
        QueryOperations::search_inventory(
            self,
            request,
            FreshnessPolicy { ttl_seconds: 60 },
            Timestamp::new(100),
        )
        .map(|result| search_inventory_output(&result))
        .map_err(Into::into)
    }
    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<destiny_mcp::schema::InspectItemOutput, destiny_mcp::schema::McpError> {
        let result = QueryOperations::inspect_item(
            self,
            request,
            FreshnessPolicy { ttl_seconds: 60 },
            Timestamp::new(100),
        )?;
        destiny_mcp::service::inspect_item_output(&result)
    }
}

fn freshness() -> destiny_cli::runner::ReadFreshness {
    destiny_cli::runner::ReadFreshness {
        policy: FreshnessPolicy { ttl_seconds: 60 },
        now: Timestamp::new(100),
    }
}

fn mcp_dispatch(data: InventorySearchResult, tool: &str, arguments: Value) -> Value {
    let server = destiny_mcp::server::ReadToolServer::new(Box::new(Reads(data)));
    server
        .dispatch(tool, arguments.as_object().unwrap().clone())
        .unwrap()
        .structured_content
        .unwrap()
}

#[test]
fn dim_query_cli_mcp_fixture_execution_parity() {
    for (query, count) in [("", 1), ("is:arc is:locked", 1), ("-is:locked", 0)] {
        let data = fixture();
        let cli = destiny_cli::runner::run(
            &mut Reads(data.clone()),
            [
                "ghostlink",
                "search-inventory",
                query,
                "--text",
                "Helmet",
                "--json",
            ],
            freshness(),
        );
        assert_eq!(cli.exit_code, 0, "{}", cli.stderr);
        let cli: Value = serde_json::from_str(&cli.stdout).unwrap();
        let mcp = mcp_dispatch(
            data,
            "search_inventory",
            json!({"query":query,"text":"Helmet"}),
        );
        assert_eq!(cli["hits"].as_array().unwrap().len(), count);
        assert_eq!(cli["hits"], mcp["hits"]);
    }
}

#[test]
fn inspect_names_have_cli_mcp_parity_and_never_choose_fuzzy_hits() {
    use destiny_cli::runner::run;
    for (count, name, expected) in [
        (1, "  TEST   HELMET ", None),
        (2, "Test Helmet", Some("ambiguous_match")),
        (1, "Helmet", Some("item_not_found")),
        (1, "Unknown", Some("item_not_found")),
    ] {
        let mut data = fixture();
        data.resolved = Default::default();
        if count == 2 {
            let mut duplicate = data.hits[0].clone();
            duplicate.instance.instance_id = ItemInstanceId::new("456");
            data.hits.push(duplicate);
        }
        let mut fuzzy = data.hits[0].clone();
        fuzzy.definition.name = "Advanced Test Helmet".into();
        fuzzy.instance.instance_id = ItemInstanceId::new("001");
        data.hits.insert(0, fuzzy);
        let cli = run(
            &mut Reads(data.clone()),
            ["ghostlink", "inspect-item", "--name", name, "--json"],
            freshness(),
        );
        let mcp = mcp_dispatch(data, "inspect_item", json!({"name":name}));
        if let Some(code) = expected {
            let cli: Value = serde_json::from_str(&cli.stderr).unwrap();
            assert_eq!(cli["error"]["code"], code);
            assert_eq!(mcp["code"], code);
            if code == "ambiguous_match" {
                assert_eq!(cli["error"]["candidates"], mcp["candidates"]);
                assert_eq!(mcp["candidates"].as_array().unwrap().len(), 2);
                assert!(mcp["candidates"][0].get("instance").is_none());
            }
        } else {
            assert_eq!(cli.exit_code, 0);
            let cli: Value = serde_json::from_str(&cli.stdout).unwrap();
            assert_eq!(cli["instance"]["instance_id"], "123");
            assert_eq!(mcp["instance"]["instance_id"], "123");
        }
    }
    for invalid in [
        json!({}),
        json!({"name":"Test Helmet","instance_id":"123"}),
        json!({"name":""}),
        json!({"name":"bad\u{7}"}),
    ] {
        let result = mcp_dispatch(fixture(), "inspect_item", invalid);
        assert_eq!(result["code"], "provider_validation");
    }
}

#[test]
fn inspect_exact_name_is_not_hidden_by_a_full_page_of_fuzzy_matches() {
    use destiny_cli::runner::run;
    let mut data = fixture();
    let exact = data.hits[0].clone();
    data.hits = (0..1000)
        .map(|index| {
            let mut fuzzy = exact.clone();
            fuzzy.definition.name = "Advanced Test Helmet".into();
            fuzzy.instance.instance_id = ItemInstanceId::new(format!("fuzzy-{index}"));
            fuzzy
        })
        .collect();
    data.hits.push(exact);
    let cli = run(
        &mut Reads(data.clone()),
        [
            "ghostlink",
            "inspect-item",
            "--name",
            "Test Helmet",
            "--json",
        ],
        freshness(),
    );
    assert_eq!(cli.exit_code, 0);
    let cli: Value = serde_json::from_str(&cli.stdout).unwrap();
    let mcp = mcp_dispatch(data, "inspect_item", json!({"name":"Test Helmet"}));
    assert_eq!(cli["instance"]["instance_id"], "123");
    assert_eq!(mcp["instance"]["instance_id"], "123");
}

#[test]
fn pagination_arguments_and_results_have_cli_mcp_parity() {
    use destiny_cli::runner::run;
    let mut data = fixture();
    data.resolved = Default::default();
    let mut second = data.hits[0].clone();
    second.instance.instance_id = ItemInstanceId::new("456");
    data.hits.push(second);
    for detail in [false, true] {
        for offset in 0..=2 {
            let mut arguments = vec![
                "ghostlink".to_owned(),
                "search-inventory".to_owned(),
                "--text".to_owned(),
                "Helmet".to_owned(),
                "--limit".to_owned(),
                "1".to_owned(),
                "--offset".to_owned(),
                offset.to_string(),
                "--json".to_owned(),
            ];
            if detail {
                arguments.push("--detail".to_owned());
            }
            let cli = run(&mut Reads(data.clone()), arguments, freshness());
            assert_eq!(cli.exit_code, 0);
            let cli: Value = serde_json::from_str(&cli.stdout).unwrap();
            let mcp = mcp_dispatch(
                data.clone(),
                "search_inventory",
                json!({"text":"Helmet","limit":1,"offset":offset,"detail":detail}),
            );
            if detail {
                // Legacy detail has transport-specific fields; compare its U1 tiles.
                assert_eq!(
                    destiny_output::search::compact_search(cli.clone())["hits"],
                    destiny_output::search::compact_search(mcp.clone())["hits"]
                );
            } else {
                assert_eq!(cli["hits"], mcp["hits"]);
            }
            assert_eq!(cli.get("next_offset"), mcp.get("next_offset"));
            assert_eq!(cli.get("next_offset").is_some(), offset == 0);
        }
    }
    let cli = run(
        &mut Reads(data.clone()),
        [
            "ghostlink",
            "search-inventory",
            "--text",
            "Helmet",
            "--offset",
            "10001",
            "--json",
        ],
        freshness(),
    );
    assert_ne!(cli.exit_code, 0);
    let mcp = mcp_dispatch(
        data,
        "search_inventory",
        json!({"text":"Helmet","offset":10001}),
    );
    assert_eq!(mcp["code"], "provider_validation");
}

#[test]
fn continuation_metadata_and_hint_match_in_both_modes() {
    let mut result = fixture();
    result.next_offset = Some(100);
    for detail in [false, true] {
        let cli = if detail {
            render_search_detail(OutputMode::Json, &result)
        } else {
            render_search(OutputMode::Json, &result)
        };
        let cli: Value = serde_json::from_str(&cli).unwrap();
        let mcp = search_inventory_output(&result);
        let mcp = if detail {
            mcp.into_detail_call_tool_result()
        } else {
            mcp.into_call_tool_result()
        };
        assert_eq!(cli["next_offset"], 100);
        assert_eq!(
            cli["next_offset"],
            mcp.structured_content.unwrap()["next_offset"]
        );
        let human = if detail {
            render_search_detail(OutputMode::Human, &result)
        } else {
            render_search(OutputMode::Human, &result)
        };
        assert!(human.contains("More results: rerun with --offset 100"));
    }
    result.next_offset = None;
    assert!(
        search_inventory_output(&result)
            .into_call_tool_result()
            .structured_content
            .unwrap()
            .get("next_offset")
            .is_none()
    );
}

#[test]
fn compact_hits_match_in_both_transports_and_detail_is_unchanged() {
    let mut result = fixture();
    for case in 0..6 {
        match case {
            1 => result.resolved = Default::default(),
            2 => {
                result.hits[0].definition.element = None;
                result.hits[0].instance.power = None;
                result.hits[0].instance.quantity = 2;
                result.hits[0].mod_names.clear();
                result.hits[0].instance.location =
                    ItemLocation::Character(CharacterId::new("hunter"));
            }
            3 => {
                result.hits[0].instance.location =
                    ItemLocation::Postmaster(CharacterId::new("hunter"))
            }
            4 => result.hits[0].instance.location = ItemLocation::other("opaque-provider-location"),
            5 => {
                result.hits[0].definition.item_type = None;
                result.hits[0].definition.tier = None;
            }
            _ => {}
        }
        let cli: Value = serde_json::from_str(&render_search(OutputMode::Json, &result)).unwrap();
        let mcp = search_inventory_output(&result)
            .into_call_tool_result()
            .structured_content
            .unwrap();
        assert_eq!(cli["hits"], mcp["hits"]);
        let detail_cli: Value =
            serde_json::from_str(&render_search_detail(OutputMode::Json, &result)).unwrap();
        let full = search_inventory_output(&result);
        let legacy_bytes = serde_json::to_value(&full).unwrap().to_string();
        let detail_mcp = full
            .into_detail_call_tool_result()
            .structured_content
            .unwrap();
        assert_eq!(detail_mcp.to_string(), legacy_bytes);
        assert_eq!(cli["candidates"], detail_cli["candidates"]);
        assert_eq!(mcp["candidates"], detail_mcp["candidates"]);
        for forbidden in [
            "definition",
            "instance",
            "available_perks",
            "available_mods",
            "archetype",
            "description",
        ] {
            assert!(cli["hits"][0].get(forbidden).is_none(), "{forbidden}");
        }
        assert_eq!(cli["hits"][0]["perk_names"], json!(["Plugged Perk"]));
        if case == 0 {
            assert_eq!(
                cli["hits"][0]["stats"],
                json!([{"name":"Mobility","value":30},{"name":"Recovery","value":20}])
            );
            assert!(cli["hits"][0].get("quantity").is_none());
            let compact_bytes = cli["hits"][0].to_string().len();
            let detail_bytes = detail_mcp["hits"][0].to_string().len();
            assert!(compact_bytes * 3 < detail_bytes * 2);
            // Pin measured sizes to make payload growth visible.
            assert_eq!((compact_bytes, detail_bytes), (310, 617));
        } else {
            assert!(cli["hits"][0].get("stats").is_none());
        }
        if case >= 2 {
            for omitted in ["element", "power", "mods"] {
                assert!(cli["hits"][0].get(omitted).is_none());
            }
            assert_eq!(cli["hits"][0]["quantity"], 2);
        }
    }
}
