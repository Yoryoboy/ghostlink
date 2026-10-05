use destiny_app::agents::{Agent, AgentReport, IntegrationError, Outcome};
use destiny_cli::command::{AgentsCommand, Command};
use destiny_cli::{OutputMode, parse_process};

#[test]
fn parsing_requires_explicit_supported_selection() {
    for verb in ["install", "uninstall"] {
        assert!(parse_process(["ghostlink", "agents", verb]).is_err());
        assert!(parse_process(["ghostlink", "agents", verb, "--agent", "other"]).is_err());
        let cli = parse_process([
            "ghostlink",
            "agents",
            verb,
            "--agent",
            "claude",
            "pi",
            "--agent",
            "codex",
            "--skill",
            "--json",
        ])
        .unwrap();
        assert_eq!(cli.output_mode(), OutputMode::Json);
        let Command::Agents(args) = cli.command else {
            panic!("agents command");
        };
        let selection = match args.command {
            AgentsCommand::Install(s) | AgentsCommand::Uninstall(s) => s,
            _ => panic!("selected action"),
        };
        assert_eq!(selection.agent.len(), 3);
        assert!(selection.skill);
        assert!(!selection.mcp);
    }
    assert!(parse_process(["ghostlink", "agents", "status", "--json"]).is_ok());
}
#[test]
fn stable_json_shape_exit_codes_and_streams() {
    let mut reports = vec![AgentReport {
        agent: Agent::Pi,
        detected: true,
        skill: Some(Outcome::Installed),
        mcp: Some(Outcome::ManualStepRequired {
            snippet: "snippet".into(),
        }),
    }];
    let output = destiny_cli::agents::render(&reports, OutputMode::Json);
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind":"agents","agents":[{
        "agent":"pi","detected":true,"skill":{"status":"installed"},
        "mcp":{"status":"manual_step_required","snippet":"snippet"}}]})
    );
    reports[0].mcp = Some(Outcome::Failed(IntegrationError::Command {
        agent: Agent::Pi,
        exit_status: Some(7),
    }));
    let output = destiny_cli::agents::render(&reports, OutputMode::Json);
    assert_eq!(output.exit_code, 51);
    assert!(output.stdout.is_empty());
    let json: serde_json::Value = serde_json::from_str(&output.stderr).unwrap();
    assert_eq!(
        json["agents"][0]["mcp"]["error"],
        serde_json::json!({"kind":"command","agent":"pi","exit_status":7})
    );
    assert!(
        destiny_cli::agents::render(&reports, OutputMode::Human)
            .stderr
            .contains("pi: detected")
    );
}
