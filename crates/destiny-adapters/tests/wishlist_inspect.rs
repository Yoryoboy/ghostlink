//! Inspect `wishlist` through the real application service, CLI and MCP views,
//! and the `is:wishlist`/`is:trashlist` search filters that share its verdict.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::QueuedProfileGateway;
use destiny_app::{ProfileSyncService, QueryService};
use destiny_core::domain::ItemSlot;
use destiny_core::wishlist::{WishlistRepository, parse_wishlist};
use destiny_core::*;

const GUN: i64 = 10;
const HELMET: i64 = 20;
const OTHER_GUN: i64 = 11;

fn definition(hash: i64, name: &str, slot: Option<ItemSlot>) -> ItemDefinition {
    ItemDefinition {
        armor: Default::default(),
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.into(),
        item_type: None,
        tier: Some("Legendary".into()),
        element: None,
        slot,
        class_type: None,
        plug_category: if slot.is_none() {
            Some("frames".into())
        } else {
            None
        },
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    }
}

fn instance(id: &str, item: i64, plugs: &[i64], available: &[i64]) -> ItemInstance {
    ItemInstance {
        energy: None,
        gear_tier: None,
        sockets: Vec::new(),
        transfer_status: None,
        available_plugs: available
            .iter()
            .map(|hash| AvailablePlug {
                socket_index: 3,
                plug_hash: ItemHash::new(*hash),
            })
            .collect(),
        instance_id: id.into(),
        item_hash: ItemHash::new(item),
        location: ItemLocation::Vault,
        quantity: 1,
        equipped: false,
        locked: false,
        power: None,
        perks: vec![],
        plugs: plugs
            .iter()
            .map(|hash| PlugSummary {
                plug_hash: ItemHash::new(*hash),
                enabled: true,
            })
            .collect(),
        stats: vec![],
    }
}

/// A gun with Outlaw socketed and an enhanced Rampage (its own hash, same name)
/// selectable; the wish list names the base Rampage hash. Plug 77 is selectable
/// but has no definition, as with a perk newer than the stored Manifest.
fn store() -> SqliteStore {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![
                    definition(GUN, "Gun", Some(ItemSlot::Kinetic)),
                    definition(HELMET, "Helmet", Some(ItemSlot::Helmet)),
                    definition(OTHER_GUN, "Other Gun", Some(ItemSlot::Energy)),
                    definition(1, "Outlaw", None),
                    definition(2, "Rampage", None),
                    definition(21, "Rampage", None),
                ],
            },
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        })
        .unwrap();
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
            items: vec![
                instance("gun", GUN, &[1], &[21, 77]),
                instance("helmet", HELMET, &[], &[]),
                instance("other", OTHER_GUN, &[2], &[]),
            ],
        })
        .unwrap();
    store
}

fn publish(store: &mut SqliteStore, key: &str, text: &str) {
    store
        .publish_wishlist(key, &parse_wishlist(text), None, "fp", Timestamp::new(1))
        .unwrap();
}

fn inspect(store: SqliteStore, id: &str) -> Resolution<ItemInspection> {
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    app.inspect_item(
        &destiny_app::InspectItemRequest {
            instance_id: id.into(),
        },
        FreshnessPolicy { ttl_seconds: 300 },
        Timestamp::new(1),
    )
    .unwrap()
}

fn views(
    inspection: &Resolution<ItemInspection>,
) -> (serde_json::Value, serde_json::Value, String) {
    let Resolution::Unique(found) = inspection else {
        panic!("expected one inspection");
    };
    let json = serde_json::from_str(&destiny_cli::output::render_inspection(
        destiny_cli::output::OutputMode::Json,
        found,
    ))
    .unwrap();
    let mcp = serde_json::to_value(destiny_mcp::service::inspect_item_output(inspection).unwrap())
        .unwrap();
    let human =
        destiny_cli::output::render_inspection(destiny_cli::output::OutputMode::Human, found);
    (json, mcp, human)
}

#[test]
fn a_matching_weapon_cites_every_list_with_enhanced_perks_matched_by_name() {
    let mut store = store();
    publish(
        &mut store,
        "voltron",
        "title:PvE Picks\ndimwishlist:item=10&perks=1,2#notes:Great add clear.|tags:pve,god-pve\n",
    );
    publish(
        &mut store,
        "/lists/mine.txt",
        "dimwishlist:item=-10#notes:Too slow for me\n",
    );
    store
        .retain_wishlist_sources(&["voltron".into(), "/lists/mine.txt".into()])
        .unwrap();
    let inspection = inspect(store, "gun");
    let (json, mcp, human) = views(&inspection);
    let expected = serde_json::json!({
        "verdict": "wishlist",
        "matches_total": 2,
        "matches": [
            {
                "source": "voltron",
                "section": "PvE Picks",
                "perks": ["Outlaw", "Rampage"],
                "notes": "Great add clear.",
                "tags": ["pve", "pve-god"]
            },
            {
                "source": "/lists/mine.txt",
                "section": null,
                "perks": [],
                "notes": "Too slow for me",
                "tags": []
            }
        ]
    });
    assert_eq!(json["wishlist"], expected);
    assert_eq!(mcp["wishlist"], expected);
    assert!(
        human.contains(
            "\nwishlist: wishlist (2 matching)\n\
             wishlist match: voltron | PvE Picks | perks: Outlaw, Rampage | notes: Great add clear. | tags: pve, pve-god\n\
             wishlist match: /lists/mine.txt | notes: Too slow for me"
        ),
        "{human}"
    );
}

/// A plug without a definition still matches its exact hash; its name stays
/// numeric because nothing names it.
#[test]
fn a_plug_without_a_definition_matches_by_hash() {
    let mut store = store();
    publish(&mut store, "voltron", "dimwishlist:item=10&perks=1,77\n");
    let (json, mcp, _) = views(&inspect(store, "gun"));
    let expected = serde_json::json!({
        "verdict": "wishlist",
        "matches_total": 1,
        "matches": [
            {"source": "voltron", "section": null, "perks": ["Outlaw", "77"], "notes": null, "tags": []}
        ]
    });
    assert_eq!(json["wishlist"], expected);
    assert_eq!(mcp["wishlist"], expected);
}

#[test]
fn an_unmatched_weapon_reports_no_opinion() {
    let mut store = store();
    publish(&mut store, "voltron", "dimwishlist:item=10&perks=1,99\n");
    let inspection = inspect(store, "gun");
    let (json, mcp, human) = views(&inspection);
    let expected = serde_json::json!({"verdict": "none", "matches_total": 0, "matches": []});
    assert_eq!(json["wishlist"], expected);
    assert_eq!(mcp["wishlist"], expected);
    assert!(human.contains("\nwishlist: none (0 matching)"), "{human}");
}

#[test]
fn the_field_is_absent_without_lists_and_for_armor() {
    let (json, mcp, human) = views(&inspect(store(), "gun"));
    assert!(json.get("wishlist").is_none());
    assert!(mcp.get("wishlist").is_none());
    assert!(!human.contains("wishlist"), "{human}");

    let mut store = store();
    publish(&mut store, "voltron", "dimwishlist:item=20&perks=1\n");
    let (json, mcp, human) = views(&inspect(store, "helmet"));
    assert!(json.get("wishlist").is_none());
    assert!(mcp.get("wishlist").is_none());
    assert!(!human.contains("wishlist"), "{human}");
}

fn search(store: SqliteStore, input: &str) -> Result<Vec<String>, AppError> {
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let instances = InstanceSearchQuery {
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
        limit: 10,
    };
    let definitions = DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 10,
    };
    let request = destiny_app::SearchInventoryRequest::new(definitions, Some(instances))
        .with_query(input)
        .unwrap();
    let result = app.search_inventory(
        &request,
        FreshnessPolicy { ttl_seconds: 300 },
        Timestamp::new(1),
    )?;
    let mut ids: Vec<String> = result
        .hits
        .iter()
        .map(|hit| hit.instance.instance_id.as_str().to_owned())
        .collect();
    ids.sort();
    Ok(ids)
}

/// The filters use the inspect verdict: the first matching roll decides, the
/// enhanced Rampage matches the base hash by name, and the gun voltron has no
/// opinion on is neither wished nor trashed.
#[test]
fn search_filters_follow_the_inspect_verdict() {
    let lists = || {
        let mut store = store();
        publish(&mut store, "voltron", "dimwishlist:item=10&perks=1,2\n");
        publish(&mut store, "/lists/mine.txt", "dimwishlist:item=-11\n");
        store
            .retain_wishlist_sources(&["voltron".into(), "/lists/mine.txt".into()])
            .unwrap();
        store
    };
    for (input, expected) in [
        ("is:wishlist", vec!["gun"]),
        ("is:trashlist", vec!["other"]),
        ("is:wishlist or is:trashlist", vec!["gun", "other"]),
        ("is:weapon -is:trashlist", vec!["gun"]),
        ("-is:wishlist", vec!["helmet", "other"]),
    ] {
        assert_eq!(search(lists(), input).unwrap(), expected, "{input}");
    }
}

/// With no list loaded the filters fail closed, like stat filters without stat
/// definitions, instead of reporting that nothing is wish-listed.
#[test]
fn wishlist_filters_need_a_loaded_list() {
    for input in ["is:wishlist", "is:trashlist", "-is:wishlist"] {
        assert_eq!(
            search(store(), input).unwrap_err().category(),
            ErrorCategory::DefinitionsUnavailable,
            "{input}"
        );
    }
    assert_eq!(search(store(), "is:weapon").unwrap(), vec!["gun", "other"]);
}
