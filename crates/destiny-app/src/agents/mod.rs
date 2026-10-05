//! Non-interactive agent integration plans and isolated step execution.
use std::ffi::OsString;
use std::path::Path;

pub use destiny_core::agents::{
    Action, Agent, AgentFiles, CommandOutput, CommandRunner, IntegrationError, Outcome, PathLookup,
    mcp_filename,
};
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub action: Action,
    pub skill: bool,
    pub mcp: bool,
}
#[derive(Clone, Debug)]
pub struct AgentReport {
    pub agent: Agent,
    pub detected: bool,
    pub skill: Option<Outcome>,
    pub mcp: Option<Outcome>,
}
fn arguments(agent: Agent, verb: &str, executable: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["mcp", verb].into_iter().map(Into::into).collect();
    if agent == Agent::Claude && verb != "list" {
        args.extend(["--scope", "user"].map(Into::into));
    }
    if verb != "list" {
        args.push("ghostlink".into());
    }
    if let Some(executable) = executable {
        args.push("--".into());
        args.push(executable.as_os_str().into());
    }
    args
}
fn checked(output: CommandOutput, agent: Agent) -> Result<CommandOutput, IntegrationError> {
    if output.success {
        Ok(output)
    } else {
        Err(IntegrationError::Command {
            agent,
            exit_status: output.exit_status,
        })
    }
}
fn cli_mcp(
    agent: Agent,
    action: Action,
    files: &dyn AgentFiles,
    lookup: &dyn PathLookup,
    runner: &mut dyn CommandRunner,
) -> Result<Outcome, IntegrationError> {
    // Resolve the sibling first; never substitute a PATH binary for destiny-mcp.
    let executable = if action == Action::Install {
        Some(files.mcp_executable()?)
    } else {
        None
    };
    if action == Action::Status
        && lookup.resolve(agent.name()).is_none()
        && !files.config_exists(agent)
    {
        return Ok(Outcome::NotInstalled);
    }
    let program = lookup
        .resolve(agent.name())
        .ok_or(IntegrationError::Command {
            agent,
            exit_status: None,
        })?;
    if action != Action::Install {
        let list = checked(
            runner.run(agent, &program, &arguments(agent, "list", None))?,
            agent,
        )?;
        let present = list
            .output
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .any(|s| s == "ghostlink");
        if !present {
            return Ok(Outcome::NotInstalled);
        }
        if action == Action::Status {
            return Ok(Outcome::AlreadyInstalled);
        }
        checked(
            runner.run(agent, &program, &arguments(agent, "remove", None))?,
            agent,
        )?;
        return Ok(Outcome::Removed);
    }
    let args = arguments(agent, "add", executable.as_deref());
    let add = runner.run(agent, &program, &args)?;
    if add.success {
        return Ok(Outcome::Installed);
    }
    // Only the explicit duplicate diagnostic permits replacing this named registration.
    if !add.output.to_ascii_lowercase().contains("already exists") {
        return checked(add, agent).map(|_| Outcome::Installed);
    }
    checked(
        runner.run(agent, &program, &arguments(agent, "remove", None))?,
        agent,
    )?;
    checked(runner.run(agent, &program, &args)?, agent)?;
    Ok(Outcome::AlreadyInstalled)
}
/// Each step is isolated: a failed skill never prevents MCP or later agents.
pub fn execute(
    agents: &[Agent],
    plan: Plan,
    files: &mut dyn AgentFiles,
    lookup: &dyn PathLookup,
    runner: &mut dyn CommandRunner,
) -> Vec<AgentReport> {
    agents
        .iter()
        .map(|&agent| {
            let detected = lookup.resolve(agent.name()).is_some() || files.config_exists(agent);
            let skill = plan.skill.then(|| {
                files
                    .skill(agent, plan.action)
                    .unwrap_or_else(Outcome::Failed)
            });
            let mcp = plan.mcp.then(|| {
                let result = if agent == Agent::OpenCode {
                    match plan.action {
                        Action::Install => files
                            .mcp_executable()
                            .and_then(|exe| files.opencode(plan.action, Some(&exe))),
                        _ => files.opencode(plan.action, None),
                    }
                } else {
                    cli_mcp(agent, plan.action, files, lookup, runner)
                };
                result.unwrap_or_else(Outcome::Failed)
            });
            AgentReport {
                agent,
                detected,
                skill,
                mcp,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_and_sibling_names() {
        assert_eq!(Agent::ALL.len(), 4);
        assert_eq!(Agent::Pi.skill_dir(), ".agents/skills");
        assert_eq!(mcp_filename(true), "destiny-mcp.exe");
        assert_eq!(mcp_filename(false), "destiny-mcp");
    }
}
