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
        destiny_app::SearchTerm::new("harmonic siphon").unwrap()
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
fn search_cli_mcp_request_parity() {
    use clap::Parser;
    use destiny_mcp::schema::SearchInventoryArgs;
    use serde_json::json;

    for (flags, args) in [
        (vec![], json!({})),
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
