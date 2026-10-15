//! Credential-free composition and output for the non-interactive engine.
use std::path::PathBuf;
use std::time::Duration;

use destiny_adapters::agents::{EnvironmentPathLookup, LocalAgentFiles, SystemCommandRunner};
use destiny_app::agents::*;
use serde_json::{Value, json};

use crate::command::{AgentName, AgentsCommand};
use crate::{OutputMode, ProcessOutput};

pub fn run_agents(
    command: &AgentsCommand,
    mode: OutputMode,
    files: &mut dyn AgentFiles,
    lookup: &dyn PathLookup,
    runner: &mut dyn CommandRunner,
) -> ProcessOutput {
    let (action, selection) = match command {
        AgentsCommand::Status => (Action::Status, None),
        AgentsCommand::Install(args) => (Action::Install, Some(args)),
        AgentsCommand::Uninstall(args) => (Action::Uninstall, Some(args)),
    };
    let mut agents = Vec::new();
    if let Some(selection) = selection {
        for name in &selection.agent {
            let agent = match name {
                AgentName::Claude => Agent::Claude,
                AgentName::Codex => Agent::Codex,
                AgentName::Opencode => Agent::OpenCode,
                AgentName::Pi => Agent::Pi,
            };
            if !agents.contains(&agent) {
                agents.push(agent);
            }
        }
    } else {
        agents.extend(Agent::ALL);
    }
    let plan = Plan {
        action,
        skill: selection.is_none_or(|s| s.skill || !s.mcp),
        mcp: selection.is_none_or(|s| s.mcp || !s.skill),
    };
    render(&execute(&agents, plan, files, lookup, runner), mode)
}
fn outcome(value: &Option<Outcome>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    let mut result = json!({"status":value.name()});
    match value {
        Outcome::ManualStepRequired { snippet } => result["snippet"] = json!(snippet),
        Outcome::Failed(error) => {
            let (kind, agent, exit_status) = match error {
                IntegrationError::Io => ("io", None, None),
                IntegrationError::ForeignSkill => ("foreign_skill", None, None),
                IntegrationError::UnsafePath => ("unsafe_path", None, None),
                IntegrationError::MissingMcpBinary => ("missing_mcp_binary", None, None),
                IntegrationError::Command { agent, exit_status } => {
                    ("command", Some(agent.name()), *exit_status)
                }
                IntegrationError::Timeout { agent } => ("timeout", Some(agent.name()), None),
            };
            result["error"] = json!({"kind":kind,"agent":agent,"exit_status":exit_status});
        }
        _ => (),
    }
    result
}
pub fn render(reports: &[AgentReport], mode: OutputMode) -> ProcessOutput {
    let failed = reports
        .iter()
        .flat_map(|r| [&r.skill, &r.mcp])
        .any(|o| matches!(o, Some(Outcome::Failed(_))));
    let exit_code = if failed { 51 } else { 0 };
    let body = if mode == OutputMode::Json {
        json!({"kind":"agents","agents":reports.iter().map(|r| json!({
            "agent":r.agent.name(),"detected":r.detected,
            "skill":outcome(&r.skill),"mcp":outcome(&r.mcp)
        })).collect::<Vec<_>>()})
        .to_string()
    } else {
        reports
            .iter()
            .map(|r| {
                let detected = if r.detected {
                    "detected"
                } else {
                    "not detected"
                };
                let mut text = format!("{}: {detected}", r.agent.name());
                for (name, value) in [("skill", &r.skill), ("mcp", &r.mcp)] {
                    if let Some(value) = value {
                        text.push_str(&format!("; {name}: {}", value.name()));
                        match value {
                            Outcome::ManualStepRequired { snippet } => {
                                text.push_str(&format!("\n{snippet}"))
                            }
                            Outcome::Failed(error) => text.push_str(&format!(" ({error:?})")),
                            _ => (),
                        }
                    }
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ProcessOutput {
        stdout: if failed {
            String::new()
        } else {
            format!("{body}\n")
        },
        stderr: if failed {
            format!("{body}\n")
        } else {
            String::new()
        },
        exit_code,
    }
}
/// No dotenv, credential file, database, or provider composition on this path.
pub fn production(command: &AgentsCommand, mode: OutputMode) -> ProcessOutput {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let executable = std::env::current_exe().ok();
    let (Some(home), Some(executable)) = (home, executable) else {
        return render(
            &[AgentReport {
                agent: Agent::Claude,
                detected: false,
                skill: Some(Outcome::Failed(IntegrationError::UnsafePath)),
                mcp: None,
            }],
            mode,
        );
    };
    run_agents(
        command,
        mode,
        &mut LocalAgentFiles::new(home, executable, cfg!(windows)),
        &EnvironmentPathLookup {
            path: std::env::var_os("PATH").unwrap_or_default(),
            windows: cfg!(windows),
        },
        &mut SystemCommandRunner {
            timeout: Duration::from_secs(15),
        },
    )
}
