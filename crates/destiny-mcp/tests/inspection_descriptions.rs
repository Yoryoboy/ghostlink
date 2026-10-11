use destiny_app::*;
use destiny_cli::output::{OutputMode, render_inspection};
use destiny_mcp::service::inspect_item_output;
use serde_json::{Value, json};

fn inspection() -> ItemInspection {
    ItemInspection {
        perk_names: vec!["Frame".into()],
        mod_names: vec!["Mod".into()],
        archetype: Some("Specialist".into()),
        available_perks: vec!["Trait".into()],
        available_mods: vec!["Alternative mod".into()],
        stats: vec![],
        plug_details: [
            (InspectionPlugRole::Perk, "Frame"),
            (InspectionPlugRole::Mod, "Mod"),
            (InspectionPlugRole::AvailablePerk, "Trait"),
            (InspectionPlugRole::AvailableMod, "Alternative mod"),
            (InspectionPlugRole::Archetype, "Specialist"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (role, name))| InspectionPlug {
            hash: ItemHash::new(i as i64 + 60),
            name: name.into(),
            role,
            description: (i != 1).then(|| "Official tooltip.\nSecond line.".into()),
            clarity: None,
            clarity_match: None,
        })
        .collect(),
        definition: ItemDefinition {
            non_transferrable: None,
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
            hash: ItemHash::new(1),
            name: "Item".into(),
            bucket_hash: None,
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: DefinitionVersion::new("v1"),
        },
        instance: ItemInstance {
            instance_id: ItemInstanceId::new("owned"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            stats: vec![],
            plugs: vec![],
            perks: vec![],
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: vec![],
        },
        metadata: QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                revision: SyncRevision::new(1),
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
                definition_version: DefinitionVersion::new("v1"),
            },
            freshness: FreshnessStatus::Fresh,
        },
    }
}

#[test]
fn mcp_inspection_descriptions_view() {
    let view =
        serde_json::to_value(inspect_item_output(&Resolution::Unique(inspection())).unwrap())
            .unwrap();
    assert_eq!(
        view["plug_details"][0],
        json!({"hash":60,"name":"Frame","role":"perk","description":"Official tooltip.\nSecond line."})
    );
    assert!(view["plug_details"][1].get("description").is_none());
    assert_eq!(view["plug_details"][2]["role"], "available_perk");
    assert_eq!(view["plug_details"][3]["role"], "available_mod");
    assert_eq!(view["plug_details"][4]["role"], "archetype");
}

fn with_intrinsic() -> ItemInspection {
    let mut inspected = inspection();
    inspected.plug_details.insert(
        0,
        InspectionPlug {
            hash: ItemHash::new(70),
            name: "Uncanny\u{1b} Arrows".into(),
            role: InspectionPlugRole::Intrinsic,
            description: Some("Official\u{1b} intrinsic.\nSecond line.".into()),
            clarity: None,
            clarity_match: None,
        },
    );
    inspected
}

#[test]
fn inspection_intrinsic_human_single_and_multiple() {
    let mut inspected = with_intrinsic();
    let human = render_inspection(OutputMode::Human, &inspected);
    assert!(
        human.contains("  intrinsic: Uncanny? Arrows\n    Official? intrinsic.\n    Second line.")
    );
    assert!(!human.contains("    Uncanny? Arrows:"));
    assert!(!human.contains('\u{1b}'));
    assert!(human.find("  intrinsic:").unwrap() < human.find("  perks:").unwrap());
    inspected.plug_details.insert(
        1,
        InspectionPlug {
            hash: ItemHash::new(71),
            name: "Another intrinsic".into(),
            role: InspectionPlugRole::Intrinsic,
            description: None,
            clarity: None,
            clarity_match: None,
        },
    );
    let human = render_inspection(OutputMode::Human, &inspected);
    assert!(human.contains("  intrinsic: Uncanny? Arrows, Another intrinsic\n    Uncanny? Arrows:\n      Official? intrinsic.\n      Second line."));
    assert!(!human.contains("    Another intrinsic:"));
}

#[test]
fn inspection_descriptions_cli_mcp_parity() {
    let mut inspected = with_intrinsic();
    for empty in [false, true] {
        if empty {
            inspected.plug_details.clear();
        }
        let cli: Value =
            serde_json::from_str(&render_inspection(OutputMode::Json, &inspected)).unwrap();
        let mcp = serde_json::to_value(
            inspect_item_output(&Resolution::Unique(inspected.clone())).unwrap(),
        )
        .unwrap();
        if empty {
            assert!(cli.get("plug_details").is_none());
            assert!(mcp.get("plug_details").is_none());
        } else {
            assert!(cli["plug_details"].is_array());
            assert_eq!(cli["plug_details"], mcp["plug_details"]);
            assert_eq!(cli["plug_details"][0]["role"], "intrinsic");
            assert_eq!(
                cli["plug_details"][0]["description"],
                "Official\u{1b} intrinsic.\nSecond line."
            );
        }
        for field in [
            "perk_names",
            "mod_names",
            "mods",
            "archetype",
            "available_perks",
            "available_mods",
            "stats",
            "instance",
        ] {
            assert_eq!(cli[field], mcp[field], "{field}");
        }
        // MCP already includes version/dataset metadata omitted by CLI.
        for field in ["definition", "metadata"] {
            for (key, value) in cli[field].as_object().unwrap() {
                assert_eq!(value, &mcp[field][key], "{field}.{key}");
            }
        }
    }
}

#[test]
fn inspection_clarity_cli_mcp_parity_and_human_all_roles() {
    let mut inspected = with_intrinsic();
    for present in [false, true] {
        for (index, plug) in inspected.plug_details.iter_mut().enumerate() {
            plug.clarity = present.then(|| "  Research\u{1b}\nSecond\n".into());
            plug.clarity_match = (present && index == 0).then(|| "variant".into());
        }
        let cli: Value =
            serde_json::from_str(&render_inspection(OutputMode::Json, &inspected)).unwrap();
        let mcp = serde_json::to_value(
            inspect_item_output(&Resolution::Unique(inspected.clone())).unwrap(),
        )
        .unwrap();
        assert_eq!(cli["plug_details"], mcp["plug_details"]);
        if present {
            assert_eq!(
                cli["plug_details"][0]["clarity"],
                "  Research\u{1b}\nSecond\n"
            );
            assert_eq!(cli["plug_details"][0]["clarity_match"], "variant");
            assert!(cli["plug_details"][1].get("clarity_match").is_none());
            let human = render_inspection(OutputMode::Human, &inspected);
            assert!(human.contains("    Clarity (variant):\n        Research?\n      Second"));
            assert_eq!(human.matches("Clarity:").count(), 5);
            assert!(!human.contains('\u{1b}'));
            assert!(!human.contains("    Uncanny? Arrows:"));
            inspected.plug_details.push(InspectionPlug {
                hash: ItemHash::new(999),
                name: "Other".into(),
                role: InspectionPlugRole::Intrinsic,
                description: None,
                clarity: Some("Other research".into()),
                clarity_match: None,
            });
            let human = render_inspection(OutputMode::Human, &inspected);
            assert!(human.contains("    Other:\n      Clarity:\n        Other research"));
        } else {
            for plug in cli["plug_details"].as_array().unwrap() {
                assert!(plug.get("clarity").is_none());
                assert!(plug.get("clarity_match").is_none());
            }
        }
    }
}

#[test]
fn inspection_descriptions_human_all_roles_and_control_safety() {
    let mut inspected = inspection();
    for plug in &mut inspected.plug_details {
        plug.description = Some("Official\u{1b} tooltip.\nSecond line.".into());
    }
    let human = render_inspection(OutputMode::Human, &inspected);
    assert_eq!(
        human
            .matches("    Official? tooltip.\n    Second line.")
            .count(),
        5
    );
    for name in ["Frame", "Mod", "Trait", "Alternative mod", "Specialist"] {
        assert!(!human.contains(&format!("    {name}:")));
    }
    assert!(!human.contains('\u{1b}'));
}
