//! Interactive setup orchestration with injectable boundaries.
use crate::ProcessOutput;
use destiny_app::agents::{Agent, AgentReport, Plan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMode {
    Both,
    Mcp,
    Skill,
}
#[derive(Debug)]
pub struct Aborted;

pub trait Prompter {
    fn agents(&mut self, detected: &[Agent]) -> Result<Vec<Agent>, Aborted>;
    fn mode(&mut self) -> Result<InstallMode, Aborted>;
    fn login(&mut self) -> Result<bool, Aborted>;
    fn show(&mut self, text: &str);
}
pub trait AgentEngine {
    fn detected(&self) -> Vec<Agent>;
    fn install(&mut self, agents: &[Agent], plan: Plan) -> Vec<AgentReport>;
}
pub trait MarkerStore {
    fn complete(&self) -> bool;
    fn write(&mut self) -> Result<(), std::io::Error>;
}
pub fn should_setup(tty: bool, json: bool, complete: bool) -> bool {
    tty && !json && !complete
}
pub fn wizard(
    p: &mut dyn Prompter,
    e: &mut dyn AgentEngine,
    m: &mut dyn MarkerStore,
    login: &mut dyn FnMut() -> ProcessOutput,
) -> ProcessOutput {
    fn flow(
        p: &mut dyn Prompter,
        e: &mut dyn AgentEngine,
        m: &mut dyn MarkerStore,
        login: &mut dyn FnMut() -> ProcessOutput,
    ) -> Result<ProcessOutput, Aborted> {
        p.show("Ghostlink setup\n");
        let detected = e.detected();
        let mut exit_code = 0;
        if detected.is_empty() {
            p.show("No supported agents detected; skipping agent installation.\n");
        } else {
            let selected = p.agents(&detected)?;
            // Defensively restrict injection results to the detected catalog.
            let selected: Vec<_> = detected
                .into_iter()
                .filter(|a| selected.contains(a))
                .collect();
            if !selected.is_empty() {
                let mode = p.mode()?;
                let reports = e.install(
                    &selected,
                    Plan {
                        action: destiny_app::agents::Action::Install,
                        skill: mode != InstallMode::Mcp,
                        mcp: mode != InstallMode::Skill,
                    },
                );
                let output = crate::agents::render(&reports, crate::OutputMode::Human);
                p.show(&output.stdout);
                p.show(&output.stderr);
                exit_code = output.exit_code;
            }
        }
        if p.login()? {
            let output = login();
            p.show(&output.stdout);
            p.show(&output.stderr);
            if output.exit_code != 0 {
                exit_code = output.exit_code;
            }
        }
        if m.write().is_err() {
            return Ok(ProcessOutput {
                stdout: String::new(),
                stderr: "Could not save setup completion marker; run ghostlink setup again.\n"
                    .into(),
                exit_code: 1,
            });
        }
        p.show(if exit_code == 0 {
            "Done. Try asking your agent: \"Which shotguns do I have?\"\n"
        } else {
            "Setup finished with errors; see the results above and retry with ghostlink setup.\n"
        });
        Ok(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
        })
    }
    flow(p, e, m, login).unwrap_or_else(|_| ProcessOutput {
        stdout: String::new(),
        stderr: "Setup cancelled; completed steps were kept.\n".into(),
        exit_code: 130,
    })
}

/// No prompting, config loading, or login composition before this gate.
pub fn usage_error(mode: crate::OutputMode) -> ProcessOutput {
    ProcessOutput::failure(
        mode,
        crate::CliError::invalid_argument(
            "setup",
            "requires interactive stdin and stdout without --json; use ghostlink agents install --agent <claude|codex|opencode|pi> [--skill] [--mcp], then ghostlink login",
        ),
    )
}

/// Injectable entry policy used by both the process boundary and tests. Bare
/// invocation falls back to clap help; explicit setup rejects unattended use.
pub fn entry(
    explicit: bool,
    tty: bool,
    mode: crate::OutputMode,
    marker: &mut dyn MarkerStore,
    start: &mut dyn FnMut(&mut dyn MarkerStore) -> ProcessOutput,
) -> ProcessOutput {
    let json = mode == crate::OutputMode::Json;
    if explicit && (!tty || json) {
        return usage_error(mode);
    }
    if explicit || should_setup(tty, json, marker.complete()) {
        return start(marker);
    }
    use clap::CommandFactory;
    ProcessOutput {
        stdout: crate::Cli::command().render_long_help().to_string(),
        stderr: String::new(),
        exit_code: 0,
    }
}

pub struct FileMarker(pub std::path::PathBuf);
impl MarkerStore for FileMarker {
    fn complete(&self) -> bool {
        self.0.is_file()
    }
    fn write(&mut self) -> Result<(), std::io::Error> {
        let parent = self
            .0
            .parent()
            .ok_or_else(|| std::io::Error::other("missing marker directory"))?;
        // Preserve PR-005's private directory convention if login was skipped
        // and this is the first operation to create the default data directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(parent)?;
        std::fs::write(&self.0, b"setup complete\n")
    }
}

pub struct IntegrationEngine<'a> {
    pub files: &'a mut dyn destiny_app::agents::AgentFiles,
    pub lookup: &'a dyn destiny_app::agents::PathLookup,
    pub runner: &'a mut dyn destiny_app::agents::CommandRunner,
}
impl AgentEngine for IntegrationEngine<'_> {
    fn detected(&self) -> Vec<Agent> {
        Agent::ALL
            .into_iter()
            .filter(|&a| self.lookup.resolve(a.name()).is_some() || self.files.config_exists(a))
            .collect()
    }
    fn install(&mut self, agents: &[Agent], plan: Plan) -> Vec<AgentReport> {
        destiny_app::agents::execute(agents, plan, self.files, self.lookup, self.runner)
    }
}

/// Defaults are explicit so the terminal and scripted prompt contracts agree.
pub fn agent_choices(detected: &[Agent]) -> (Vec<&'static str>, Vec<bool>) {
    (
        detected
            .iter()
            .map(|a| match a {
                Agent::Claude => "Claude Code",
                Agent::Codex => "Codex",
                Agent::OpenCode => "OpenCode",
                Agent::Pi => "Pi",
            })
            .collect(),
        vec![true; detected.len()],
    )
}
pub struct TerminalPrompter;
impl Prompter for TerminalPrompter {
    fn agents(&mut self, detected: &[Agent]) -> Result<Vec<Agent>, Aborted> {
        let _guard = terminal::Guard::new()?;
        let (labels, defaults) = agent_choices(detected);
        let indices = dialoguer::MultiSelect::new()
            .with_prompt(
                "Which agents should Ghostlink connect to? (space to toggle, enter to confirm)",
            )
            .items(&labels)
            .defaults(&defaults)
            .interact_on_opt(&dialoguer::console::Term::stdout())
            .map_err(|_| Aborted)?
            .ok_or(Aborted)?;
        Ok(indices.into_iter().map(|i| detected[i]).collect())
    }
    fn mode(&mut self) -> Result<InstallMode, Aborted> {
        let _guard = terminal::Guard::new()?;
        let index = dialoguer::Select::new()
            .with_prompt("Install")
            .items([
                "Skill and MCP server (recommended)",
                "MCP server only",
                "CLI skill only",
            ])
            .default(0)
            .interact_on_opt(&dialoguer::console::Term::stdout())
            .map_err(|_| Aborted)?
            .ok_or(Aborted)?;
        Ok([InstallMode::Both, InstallMode::Mcp, InstallMode::Skill][index])
    }
    fn login(&mut self) -> Result<bool, Aborted> {
        let _guard = terminal::Guard::new()?;
        dialoguer::Confirm::new()
            .with_prompt("Sign in to Bungie now?")
            .default(true)
            .interact_on_opt(&dialoguer::console::Term::stdout())
            .map_err(|_| Aborted)?
            .ok_or(Aborted)
    }
    fn show(&mut self, text: &str) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }
}

pub fn production(
    marker: &mut dyn MarkerStore,
    login: &mut dyn FnMut() -> ProcessOutput,
) -> ProcessOutput {
    use destiny_adapters::agents::{EnvironmentPathLookup, LocalAgentFiles, SystemCommandRunner};
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let (Some(home), Ok(executable)) = (home, std::env::current_exe()) else {
        return ProcessOutput {
            stdout: String::new(),
            stderr: "Could not resolve setup paths.\n".into(),
            exit_code: 1,
        };
    };
    let mut files = LocalAgentFiles::new(home.into(), executable, cfg!(windows));
    let lookup = EnvironmentPathLookup {
        path: std::env::var_os("PATH").unwrap_or_default(),
        windows: cfg!(windows),
    };
    let mut runner = SystemCommandRunner {
        timeout: std::time::Duration::from_secs(15),
    };
    wizard(
        &mut TerminalPrompter,
        &mut IntegrationEngine {
            files: &mut files,
            lookup: &lookup,
            runner: &mut runner,
        },
        marker,
        login,
    )
}

mod terminal;
