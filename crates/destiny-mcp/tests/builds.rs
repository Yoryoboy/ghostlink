use clap::Parser;
use destiny_mcp::schema;

#[test]
fn build_descriptors_are_small_closed_and_source_validation_is_strict() {
    let tools = schema::build_tool_specs();
    assert_eq!(
        tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
        [
            "list_builds",
            "get_build",
            "save_build",
            "delete_build",
            "analyze_build",
            "build_substitutes"
        ]
    );
    for tool in &tools {
        assert!(tool.description.as_ref().unwrap().len() <= 500);
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
    // The substitutes read is read-only like analyze_build.
    let substitutes = tools
        .iter()
        .find(|t| t.name == "build_substitutes")
        .unwrap();
    assert_eq!(
        substitutes.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    assert_eq!(
        substitutes.input_schema["properties"]["limit"]["maximum"],
        200
    );
    assert_eq!(
        substitutes.input_schema["properties"]["limit"]["minimum"],
        1
    );
    let args: schema::SaveBuildArgs = serde_json::from_value(serde_json::json!({
        "source":"equipped", "input":"bad", "character":"hunter", "name":"Raid"
    }))
    .unwrap();
    assert!(args.validate().is_err());
}

#[test]
fn cli_mcp_build_check_request_parity() {
    for (args, wire) in [
        (
            vec!["ghostlink", "build", "check", "id", "--character", "hunter"],
            serde_json::json!({"id":"id", "character":"hunter"}),
        ),
        (
            vec!["ghostlink", "build", "check", "--name", "Raid"],
            serde_json::json!({"name":"Raid"}),
        ),
    ] {
        let cli = destiny_cli::command::Cli::try_parse_from(args).unwrap();
        let destiny_cli::command::Command::Build(build) = cli.command else {
            panic!()
        };
        let destiny_cli::command::BuildCommand::Check(args) = build.command else {
            panic!()
        };
        let mcp: schema::AnalyzeBuildArgs = serde_json::from_value(wire).unwrap();
        assert_eq!(args.request().unwrap(), mcp.request().unwrap());
    }
    for wire in [
        serde_json::json!({}),
        serde_json::json!({"id":"id", "name":"Raid"}),
        serde_json::json!({"id":"id", "character":"\n"}),
    ] {
        assert!(
            serde_json::from_value::<schema::AnalyzeBuildArgs>(wire)
                .unwrap()
                .request()
                .is_err()
        );
    }
}

#[test]
fn cli_mcp_build_selector_parity() {
    for (args, wire) in [
        (
            vec!["ghostlink", "build", "show", "id"],
            serde_json::json!({"id":"id"}),
        ),
        (
            vec!["ghostlink", "build", "show", "--name", "Raid"],
            serde_json::json!({"name":"Raid"}),
        ),
    ] {
        let cli = destiny_cli::command::Cli::try_parse_from(args).unwrap();
        let destiny_cli::command::Command::Build(build) = cli.command else {
            panic!()
        };
        let destiny_cli::command::BuildCommand::Show(selection) = build.command else {
            panic!()
        };
        let mcp: schema::BuildSelectionArgs = serde_json::from_value(wire).unwrap();
        assert_eq!(selection.selector().unwrap(), mcp.selector().unwrap());
    }
    for wire in [
        serde_json::json!({}),
        serde_json::json!({"id":"id", "name":"Raid"}),
        serde_json::json!({"name":"\n"}),
    ] {
        let mcp: schema::BuildSelectionArgs = serde_json::from_value(wire).unwrap();
        assert!(mcp.selector().is_err());
    }
}

#[test]
fn cli_mcp_build_substitutes_request_parity_and_limit_bounds() {
    for (args, wire) in [
        (
            vec![
                "ghostlink",
                "build",
                "substitutes",
                "id",
                "--character",
                "hunter",
            ],
            serde_json::json!({"id":"id", "character":"hunter"}),
        ),
        (
            vec![
                "ghostlink",
                "build",
                "substitutes",
                "--name",
                "Raid",
                "--limit",
                "25",
            ],
            serde_json::json!({"name":"Raid", "limit":25}),
        ),
    ] {
        let cli = destiny_cli::command::Cli::try_parse_from(args).unwrap();
        let destiny_cli::command::Command::Build(build) = cli.command else {
            panic!()
        };
        let destiny_cli::command::BuildCommand::Substitutes(args) = build.command else {
            panic!()
        };
        let mcp: schema::SubstitutesArgs = serde_json::from_value(wire).unwrap();
        assert_eq!(args.request().unwrap(), mcp.request().unwrap());
    }
    for wire in [
        serde_json::json!({}),
        serde_json::json!({"id":"id", "name":"Raid"}),
        serde_json::json!({"id":"id", "limit":0}),
        serde_json::json!({"id":"id", "limit":201}),
    ] {
        assert!(
            serde_json::from_value::<schema::SubstitutesArgs>(wire)
                .unwrap()
                .request()
                .is_err()
        );
    }
    assert!(
        serde_json::from_value::<schema::SubstitutesArgs>(serde_json::json!({"id":"id", "x":1}))
            .is_err()
    );
}
