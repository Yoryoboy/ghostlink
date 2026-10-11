use clap::Parser;
use destiny_mcp::schema;

#[test]
fn build_descriptors_are_small_closed_and_source_validation_is_strict() {
    let tools = schema::build_tool_specs();
    assert_eq!(
        tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
        ["list_builds", "get_build", "save_build", "delete_build"]
    );
    for tool in tools {
        assert!(tool.description.unwrap().len() <= 500);
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
    let args: schema::SaveBuildArgs = serde_json::from_value(serde_json::json!({
        "source":"equipped", "input":"bad", "character":"hunter", "name":"Raid"
    }))
    .unwrap();
    assert!(args.validate().is_err());
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
