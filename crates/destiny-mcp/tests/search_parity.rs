//! Parse the real CLI command surface without introducing an adapter dependency.
#[allow(dead_code)]
#[path = "../../destiny-cli/src/command.rs"]
mod cli_command;

// Command parsing only needs the presentation mode discriminator.
mod output {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum OutputMode {
        Human,
        Json,
    }
}

#[path = "../../destiny-cli/src/subclass_output.rs"]
mod cli_subclass_output;

#[path = "../../destiny-cli/src/artifact_output.rs"]
mod cli_artifact_output;

#[test]
fn artifact_cli_mcp_json_parity() {
    use destiny_app::{
        ArtifactPerkView, ArtifactResult, ArtifactSummary, ArtifactTierView, CharacterArtifact,
        CharacterArtifactState, CharacterId, ItemHash,
    };
    let perk = ArtifactPerkView {
        item_hash: ItemHash::new(7),
        name: None,
        active: true,
    };
    let mut result = ArtifactResult {
        artifact: Some(ArtifactSummary {
            artifact_hash: ItemHash::new(3),
            name: Some("Artifact".into()),
            points_acquired: 12,
            power_bonus: 9,
        }),
        characters: vec![
            CharacterArtifact {
                character_id: CharacterId::new("1"),
                class: Some("Hunter".into()),
                state: Some(CharacterArtifactState {
                    points_used: 1,
                    reset_count: 2,
                    active_perks: vec![perk.clone()],
                    tiers: vec![
                        ArtifactTierView {
                            index: 1,
                            unlocked: true,
                            points_to_unlock: 0,
                            perks: vec![perk],
                        },
                        ArtifactTierView {
                            index: 2,
                            unlocked: false,
                            points_to_unlock: 5,
                            perks: vec![],
                        },
                    ],
                }),
            },
            CharacterArtifact {
                character_id: CharacterId::new("2"),
                class: None,
                state: None,
            },
            CharacterArtifact {
                character_id: CharacterId::new("3"),
                class: None,
                state: Some(CharacterArtifactState {
                    points_used: 0,
                    reset_count: 0,
                    active_perks: vec![],
                    tiers: vec![],
                }),
            },
        ],
    };
    for _ in 0..2 {
        let cli = cli_artifact_output::artifact_json(&result);
        let mcp = destiny_mcp::service::artifact_output(&result).into_call_tool_result();
        assert_eq!(mcp.structured_content.unwrap(), cli);
        result.artifact = None;
        result.characters.clear();
    }
}

#[test]
fn artifact_tool_has_closed_bounded_arguments() {
    use destiny_mcp::schema::GetArtifactArgs;
    for selector in [None, Some("123"), Some("hUnTeR")] {
        use clap::Parser;
        let mut argv = vec!["ghostlink", "--json", "artifact"];
        if let Some(value) = selector {
            argv.extend(["--character", value]);
        }
        let cli = cli_command::Cli::try_parse_from(argv)
            .unwrap()
            .into_read_request()
            .unwrap();
        let cli_command::ReadRequest::Artifact {
            character_id,
            tiers,
        } = cli
        else {
            panic!("artifact request")
        };
        assert!(!tiers);
        assert_eq!(
            character_id,
            GetArtifactArgs {
                character: selector.map(str::to_owned)
            }
            .into_request()
            .unwrap()
        );
    }
    for value in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(513),
        "bad\u{7}".to_owned(),
    ] {
        assert!(
            GetArtifactArgs {
                character: Some(value)
            }
            .into_request()
            .is_err()
        );
    }
    assert!(
        GetArtifactArgs {
            character: Some("a".repeat(512))
        }
        .into_request()
        .is_ok()
    );
    for value in [
        serde_json::json!({"unknown": 1}),
        serde_json::json!({"character": 1}),
    ] {
        assert!(serde_json::from_value::<GetArtifactArgs>(value).is_err());
    }
    let tool = destiny_mcp::schema::tool_specs()
        .into_iter()
        .find(|tool| tool.name == "get_artifact")
        .expect("artifact tool registered");
    let schema = tool.schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert!(
        schema["properties"]["character"]
            .to_string()
            .contains("512")
    );
    assert!(tool.output_schema.is_some());
}

#[test]
fn subclass_cli_mcp_json_parity() {
    use destiny_app::{
        CharacterId, CharacterSubclass, ItemHash, ItemInstanceId, SubclassConfig,
        SubclassPlugKind::*, SubclassResult, SubclassSlot,
    };
    let mut result = SubclassResult {
        characters: vec![
            CharacterSubclass {
                character_id: CharacterId::new("1"),
                class: Some("Hunter".into()),
                subclass: Some(SubclassConfig {
                    instance_id: ItemInstanceId::new("2"),
                    item_hash: ItemHash::new(3),
                    name: "Prismatic".into(),
                    element: Some("Prismatic".into()),
                    slots: [
                        Fragment,
                        Super,
                        Aspect,
                        Grenade,
                        Melee,
                        ClassAbility,
                        Movement,
                        Transcendence,
                        Fragment,
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(i, kind)| SubclassSlot {
                        kind,
                        plug_hash: ItemHash::new(i as i64),
                        name: (i != 0).then(|| format!("Plug {i}")),
                        empty: i == 8,
                    })
                    .collect(),
                }),
            },
            CharacterSubclass {
                character_id: CharacterId::new("4"),
                class: None,
                subclass: None,
            },
        ],
    };
    for _ in 0..2 {
        let cli = cli_subclass_output::subclass_json(&result);
        let mcp = destiny_mcp::service::subclass_output(&result).into_call_tool_result();
        assert_eq!(mcp.structured_content.unwrap(), cli);
        result.characters.clear();
    }
}

#[test]
fn subclass_cli_mcp_selector_parity_and_invalid_arguments() {
    use clap::Parser;
    use destiny_mcp::schema::GetSubclassArgs;
    for selector in [None, Some("123"), Some("hUnTeR")] {
        let mut argv = vec!["ghostlink", "subclass"];
        if let Some(value) = selector {
            argv.extend(["--character", value]);
        }
        let cli = cli_command::Cli::try_parse_from(argv)
            .unwrap()
            .into_read_request()
            .unwrap();
        let cli_command::ReadRequest::Subclass(character) = cli else {
            panic!("subclass request")
        };
        let mcp: GetSubclassArgs =
            serde_json::from_value(serde_json::json!({"character": selector})).unwrap();
        assert_eq!(character, mcp.into_request().unwrap());
    }
    for value in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(513),
        "bad\u{7}".to_owned(),
    ] {
        assert!(
            GetSubclassArgs {
                character: Some(value)
            }
            .into_request()
            .is_err()
        );
    }
    assert!(
        GetSubclassArgs {
            character: Some("a".repeat(512))
        }
        .into_request()
        .is_ok()
    );
    assert!(serde_json::from_value::<GetSubclassArgs>(serde_json::json!({"unknown": 1})).is_err());
}

#[test]
fn subclass_tool_has_closed_bounded_schema() {
    let tool = destiny_mcp::schema::tool_specs()
        .into_iter()
        .find(|tool| tool.name == "get_subclass")
        .expect("subclass tool is registered");
    let schema = tool.schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert!(
        schema["properties"]["character"]
            .to_string()
            .contains("512")
    );
    assert!(tool.output_schema.is_some());
}

#[test]
fn sort_and_limit_cli_mcp_parity() {
    use clap::Parser;
    for sort in ["Range", "Range:asc", "Range:desc"] {
        let cli = cli_command::Cli::try_parse_from([
            "ghostlink",
            "search-inventory",
            "--sort",
            sort,
            "--limit",
            "1000",
        ])
        .unwrap()
        .into_read_request()
        .unwrap();
        let cli_command::ReadRequest::SearchInventory(cli) = cli else {
            panic!("search")
        };
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"sort": sort, "limit": 1000})).unwrap();
        assert_eq!(cli, mcp.into_request().unwrap());
    }
    for flags in [["--sort", "Range:up"], ["--limit", "1001"]] {
        let cli =
            cli_command::Cli::try_parse_from(["ghostlink", "search-inventory", flags[0], flags[1]])
                .unwrap();
        let error = cli.into_read_request().unwrap_err().to_string();
        assert!(error.contains(flags[0]));
        let args = if flags[0] == "--sort" {
            serde_json::json!({"sort": flags[1]})
        } else {
            serde_json::json!({"limit": 1001})
        };
        let mcp: destiny_mcp::schema::SearchInventoryArgs = serde_json::from_value(args).unwrap();
        assert!(mcp.into_request().is_err());
    }
    for limit in [1000, 1001] {
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"instances": {"limit": limit}})).unwrap();
        assert_eq!(mcp.into_request().is_ok(), limit == 1000);
    }
    let schema = serde_json::to_value(schemars::schema_for!(
        destiny_mcp::schema::SearchInventoryArgs
    ))
    .unwrap();
    assert!(schema["properties"]["sort"].is_object());
    assert_eq!(schema["properties"]["limit"]["maximum"], 1000);
    assert_eq!(
        schema["$defs"]["InstanceQueryArgs"]["properties"]["limit"]["maximum"],
        1000
    );
}

#[test]
fn tier_location_equipped_cli_mcp_parity() {
    use clap::Parser;
    for (flags, args) in [
        (
            vec!["--tier", "eXoTiC"],
            serde_json::json!({"tier": "eXoTiC"}),
        ),
        (
            vec!["--tier", "imaginary"],
            serde_json::json!({"tier": "imaginary"}),
        ),
        (
            vec!["--location", "vault"],
            serde_json::json!({"location": "vault"}),
        ),
        (
            vec!["--equipped", "false"],
            serde_json::json!({"equipped": false}),
        ),
        (
            vec!["--equipped", "true"],
            serde_json::json!({"equipped": true}),
        ),
        (
            vec!["--location", "character", "--equipped", "true"],
            serde_json::json!({"location": "character", "equipped": true}),
        ),
        (
            vec!["--location", "vault", "--character", "123"],
            serde_json::json!({"location": "vault", "instances": {"character_id": "123"}}),
        ),
    ] {
        let mut argv = vec!["ghostlink", "search-inventory"];
        argv.extend(flags);
        let cli = cli_command::Cli::try_parse_from(argv)
            .unwrap()
            .into_read_request()
            .unwrap();
        let cli_command::ReadRequest::SearchInventory(cli) = cli else {
            panic!("search request")
        };
        let mcp: destiny_mcp::schema::SearchInventoryArgs = serde_json::from_value(args).unwrap();
        assert!(cli.instances.is_some());
        assert_eq!(cli, mcp.into_request().unwrap());
    }
}

#[test]
fn invalid_location_and_equipped_name_the_argument() {
    use clap::Parser;
    for (flag, value, allowed) in [
        ("--location", "orbit", "vault"),
        ("--equipped", "yes", "true"),
    ] {
        let cli = cli_command::Cli::try_parse_from(["ghostlink", "search-inventory", flag, value])
            .unwrap();
        let error = cli.into_read_request().unwrap_err();
        assert!(error.to_string().contains(flag));
        assert!(error.to_string().contains(allowed));
    }
    let mcp: destiny_mcp::schema::SearchInventoryArgs =
        serde_json::from_value(serde_json::json!({"location": "orbit"})).unwrap();
    assert_eq!(
        mcp.into_request().unwrap_err().message(),
        "invalid location: expected one of vault, character"
    );
    assert!(
        serde_json::from_value::<destiny_mcp::schema::SearchInventoryArgs>(
            serde_json::json!({"equipped": "true"})
        )
        .is_err()
    );
}

#[test]
fn filter_schema_exposes_tier_location_and_equipped() {
    let schema = serde_json::to_value(schemars::schema_for!(
        destiny_mcp::schema::SearchInventoryArgs
    ))
    .unwrap();
    assert!(schema["properties"]["tier"].is_object());
    assert!(
        schema["properties"]["equipped"]
            .to_string()
            .contains("boolean")
    );
    assert!(
        schema["properties"]["location"]
            .to_string()
            .contains("LocationSchema")
    );
    assert_eq!(
        schema["$defs"]["LocationSchema"]["enum"],
        serde_json::json!(["vault", "character"])
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn available_flag_builds_identical_cli_and_mcp_queries() {
    use clap::Parser;
    for enabled in [false, true] {
        let mut argv = vec!["ghostlink", "search-inventory", "--perk", "Kill Clip"];
        if enabled {
            argv.push("--include-available");
        }
        let cli = cli_command::Cli::try_parse_from(argv)
            .unwrap()
            .into_read_request()
            .unwrap();
        let cli_command::ReadRequest::SearchInventory(cli) = cli else {
            panic!("search request")
        };
        let mcp: destiny_mcp::schema::SearchInventoryArgs = serde_json::from_value(
            serde_json::json!({"perk_name": "Kill Clip", "include_available": enabled}),
        )
        .unwrap();
        assert_eq!(cli, mcp.into_request().unwrap());
        assert_eq!(cli.instances.unwrap().include_available, enabled);
    }
}

#[test]
fn strict_stat_conditions_build_identical_cli_and_mcp_queries() {
    use clap::Parser;
    for condition in ["Range>60", "Range<30"] {
        let cli = cli_command::Cli::try_parse_from([
            "ghostlink",
            "search-inventory",
            "--stat",
            condition,
        ])
        .unwrap()
        .into_read_request()
        .unwrap();
        let cli_command::ReadRequest::SearchInventory(cli) = cli else {
            panic!("search request")
        };
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"stats": [condition]})).unwrap();
        assert_eq!(cli, mcp.into_request().unwrap());
    }
}

#[test]
fn invalid_stat_conditions_are_rejected_by_both_surfaces() {
    use clap::Parser;
    for conditions in [
        vec!["Weapons>>30".to_owned()],
        vec!["=20".to_owned()],
        vec!["Health=20".to_owned(); 9],
        vec![format!("{}=1", "a".repeat(257))],
    ] {
        let mut argv = vec!["ghostlink".to_owned(), "search-inventory".to_owned()];
        for condition in &conditions {
            argv.extend(["--stat".to_owned(), condition.clone()]);
        }
        assert!(
            cli_command::Cli::try_parse_from(argv)
                .unwrap()
                .into_read_request()
                .is_err()
        );
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"stats": conditions})).unwrap();
        assert!(mcp.into_request().is_err());
    }
    assert!(
        serde_json::from_value::<destiny_mcp::schema::SearchInventoryArgs>(
            serde_json::json!({"stat": ["Weapons>=30"]})
        )
        .is_err()
    );
}

#[test]
fn invalid_perk_names_are_rejected_by_both_surfaces() {
    use clap::Parser;
    for name in [
        "".to_owned(),
        "  ".to_owned(),
        "a".repeat(257),
        "bad\u{7}".to_owned(),
    ] {
        let cli =
            cli_command::Cli::try_parse_from(["ghostlink", "search-inventory", "--perk", &name])
                .unwrap();
        assert!(cli.into_read_request().is_err());
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"perk_name": name})).unwrap();
        assert!(mcp.into_request().is_err());
    }
    assert!(
        serde_json::from_value::<destiny_mcp::schema::SearchInventoryArgs>(
            serde_json::json!({"perk": "Rampage"})
        )
        .is_err()
    );
}

#[test]
fn mod_only_search_builds_owned_query_and_rejects_invalid_names() {
    use clap::Parser;
    let cli = cli_command::Cli::try_parse_from([
        "ghostlink",
        "search-inventory",
        "--mod",
        "Harmonic Siphon",
    ])
    .unwrap()
    .into_read_request()
    .unwrap();
    let cli_command::ReadRequest::SearchInventory(request) = cli else {
        panic!("search request")
    };
    let query = request.instances.unwrap();
    assert_eq!(
        query.mod_name.unwrap(),
        vec![destiny_app::SearchTerm::new("harmonic siphon").unwrap()]
    );
    for name in [
        "".to_owned(),
        "  ".to_owned(),
        "a".repeat(257),
        "bad\u{7}".to_owned(),
    ] {
        let cli =
            cli_command::Cli::try_parse_from(["ghostlink", "search-inventory", "--mod", &name])
                .unwrap();
        assert!(cli.into_read_request().is_err());
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"mod_name": name})).unwrap();
        assert!(mcp.into_request().is_err());
    }
    assert!(
        serde_json::from_value::<destiny_mcp::schema::SearchInventoryArgs>(
            serde_json::json!({"mod": "Harmonic Siphon"})
        )
        .is_err()
    );
}

#[test]
fn archetype_only_search_builds_owned_query_and_rejects_invalid_names() {
    use clap::Parser;
    let cli = cli_command::Cli::try_parse_from([
        "ghostlink",
        "search-inventory",
        "--archetype",
        "Specialist",
    ])
    .unwrap()
    .into_read_request()
    .unwrap();
    let cli_command::ReadRequest::SearchInventory(request) = cli else {
        panic!("search request")
    };
    assert!(request.instances.is_some());
    for name in [
        "".to_owned(),
        "  ".to_owned(),
        "a".repeat(257),
        "bad\u{7}".to_owned(),
    ] {
        let cli = cli_command::Cli::try_parse_from([
            "ghostlink",
            "search-inventory",
            "--archetype",
            &name,
        ])
        .unwrap();
        assert!(cli.into_read_request().is_err());
        let mcp: destiny_mcp::schema::SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"archetype": name})).unwrap();
        assert!(mcp.into_request().is_err());
    }
    assert!(
        serde_json::from_value::<destiny_mcp::schema::SearchInventoryArgs>(
            serde_json::json!({"archetype_name": "Specialist"})
        )
        .is_err()
    );
}

#[test]
fn repeated_name_bounds_apply_to_both_surfaces_and_combined_mcp_fields() {
    use clap::Parser;
    for (flag, singular, plural, kind) in [
        ("--perk", "perk_name", "perk_names", "perk"),
        ("--mod", "mod_name", "mod_names", "mod"),
    ] {
        let mut argv = vec!["ghostlink", "search-inventory"];
        for _ in 0..9 {
            argv.extend([flag, "Rampage"]);
        }
        let cli = cli_command::Cli::try_parse_from(argv).unwrap();
        assert!(cli.into_read_request().is_err());
        for single in [false, true] {
            let mut args = serde_json::json!({plural: vec!["Rampage"; if single {8} else {9}]});
            if single {
                args[singular] = serde_json::json!("Rampage");
            }
            let args: destiny_mcp::schema::SearchInventoryArgs =
                serde_json::from_value(args).unwrap();
            let error = args.into_request().unwrap_err();
            assert_eq!(
                error.message(),
                format!("invalid {kind}: at most 8 {kind} names")
            );
        }
    }
}

#[test]
fn search_cli_mcp_request_parity() {
    use clap::Parser;
    use destiny_mcp::schema::SearchInventoryArgs;
    use serde_json::json;

    for (flags, args) in [
        (vec![], json!({})),
        (
            vec!["--perk", "Rampage", "--perk", "Rampage"],
            json!({"perk_name": "Rampage", "perk_names": ["Rampage"]}),
        ),
        (
            vec![
                "--mod",
                "Targeting",
                "--mod",
                "Siphon",
                "--include-available",
            ],
            json!({"mod_name": "Targeting", "mod_names": ["Siphon"], "include_available": true}),
        ),
        (
            vec![
                "--perk",
                "Rampage",
                "--perk",
                "Kill Clip",
                "--include-available",
            ],
            json!({"perk_names": ["Rampage", "Kill Clip"], "include_available": true}),
        ),
        (
            vec!["--mod", "Harmonic Siphon", "--mod", "Targeting"],
            json!({"mod_names": ["Harmonic Siphon", "Targeting"]}),
        ),
        (vec!["--perk", "ramp"], json!({"perk_name": "ramp"})),
        (vec!["--perk", "rampge"], json!({"perk_name": "rampge"})),
        (vec!["--mod", "siph"], json!({"mod_name": "siph"})),
        (vec!["--archetype", "spec"], json!({"archetype": "spec"})),
        (vec!["--stat", "cla>=30"], json!({"stats": ["cla>=30"]})),
        (
            vec!["--archetype", "  SPECIALIST "],
            json!({"archetype": "  SPECIALIST "}),
        ),
        (
            vec![
                "--slot",
                "helmet",
                "--archetype",
                "Specialist",
                "--stat",
                "Class>=30",
                "--limit",
                "1",
            ],
            json!({"slot": "helmet", "archetype": "Specialist", "stats": ["Class>=30"], "limit": 1}),
        ),
        (
            vec!["--stat", " Weapons >= 30 "],
            json!({"stats": [" Weapons >= 30 "]}),
        ),
        (
            vec![
                "--slot",
                "helmet",
                "--class",
                "hunter",
                "--stat",
                "Class<=35",
                "--stat",
                "Health=20",
            ],
            json!({"slot": "helmet", "class_type": "hunter", "stats": ["Class<=35", "Health=20"]}),
        ),
        (
            vec!["--mod", "  HARMONIC   SIPHON "],
            json!({"mod_name": "  HARMONIC   SIPHON "}),
        ),
        (
            vec!["--mod", "Harmonic Siphon", "--limit", "1"],
            json!({"mod_name": "Harmonic Siphon", "limit": 1}),
        ),
        (
            vec!["--mod", "Harmonic Siphon", "--perk", "Rampage"],
            json!({"mod_name": "Harmonic Siphon", "perk_name": "Rampage"}),
        ),
        (
            vec!["--mod", "Harmonic Siphon", "--character", "123"],
            json!({"mod_name": "Harmonic Siphon", "instances": {"character_id": "123"}}),
        ),
        (
            vec!["--perk", "  RAMPAGE "],
            json!({"perk_name": "  RAMPAGE "}),
        ),
        (
            vec!["--perk", "Rampage", "--limit", "1"],
            json!({"perk_name": "Rampage", "limit": 1}),
        ),
        (
            vec!["--type", "Hand Cannon"],
            json!({"item_type": "Hand Cannon"}),
        ),
        (vec!["--element", "SoLaR"], json!({"element": "SoLaR"})),
        (vec!["--slot", "Energy"], json!({"slot": "Energy"})),
        (vec!["--class", "Hunter"], json!({"class_type": "Hunter"})),
        (vec!["--min-power", "2000"], json!({"min_power": 2000})),
        (
            vec![
                "--text",
                "ace",
                "--tag",
                "exotic",
                "--element",
                "solar",
                "--slot",
                "kinetic",
                "--class",
                "any",
                "--min-power",
                "2000",
                "--item-hash",
                "1",
                "--perk-hash",
                "2",
                "--character",
                "123",
                "--limit",
                "5",
            ],
            json!({"text": "ace", "tags": ["exotic"], "element": "solar", "slot": "kinetic", "class_type": "any", "min_power": 2000, "limit": 5, "instances": {"item_hashes": [1], "perk_hash": 2, "character_id": "123", "limit": 5}}),
        ),
    ] {
        let argv = [vec!["ghostlink", "search-inventory"], flags].concat();
        let cli = cli_command::Cli::try_parse_from(argv)
            .unwrap()
            .into_read_request()
            .unwrap();
        let cli_command::ReadRequest::SearchInventory(cli) = cli else {
            panic!("search request")
        };
        let mcp: SearchInventoryArgs = serde_json::from_value(args).unwrap();
        assert_eq!(cli, mcp.into_request().unwrap());
    }
}
