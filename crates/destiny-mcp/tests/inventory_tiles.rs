//! Real CLI/MCP inventory hit parity, including legacy detail and omission rules.
use destiny_app::*;
use destiny_cli::output::{OutputMode, render_search, render_search_detail};
use destiny_mcp::service::search_inventory_output;
use serde_json::{Value, json};

fn fixture() -> InventorySearchResult {
    let definition = ItemDefinition {
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
        resolved: Default::default(),
        candidates: vec![definition.clone()],
        hits: vec![InventorySearchHit {
            definition,
            instance: ItemInstance {
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
