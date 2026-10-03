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
fn search_cli_mcp_request_parity() {
    use clap::Parser;
    use destiny_mcp::schema::SearchInventoryArgs;
    use serde_json::json;

    for (flags, args) in [
        (vec![], json!({})),
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
