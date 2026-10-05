//! Provider-free agent integration contracts shared by policy and adapters.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
    Pi,
}
impl Agent {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::OpenCode, Self::Pi];
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }
    pub fn config_dir(self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::OpenCode => ".config/opencode",
            Self::Pi => ".pi",
        }
    }
    pub fn skill_dir(self) -> &'static str {
        match self {
            Self::Claude => ".claude/skills",
            Self::Codex => ".codex/skills",
            Self::OpenCode => ".config/opencode/skills",
            Self::Pi => ".agents/skills",
        }
    }
}
pub fn mcp_filename(windows: bool) -> &'static str {
    if windows {
        "destiny-mcp.exe"
    } else {
        "destiny-mcp"
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationError {
    Io,
    ForeignSkill,
    UnsafePath,
    MissingMcpBinary,
    Command {
        agent: Agent,
        exit_status: Option<i32>,
    },
    Timeout {
        agent: Agent,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Installed,
    AlreadyInstalled,
    Removed,
    NotInstalled,
    ManualStepRequired { snippet: String },
    Failed(IntegrationError),
}
impl Outcome {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::AlreadyInstalled => "already_installed",
            Self::Removed => "removed",
            Self::NotInstalled => "not_installed",
            Self::ManualStepRequired { .. } => "manual_step_required",
            Self::Failed(_) => "failed",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Status,
    Install,
    Uninstall,
}
/// Filesystem operations are implemented by the local adapter, not application policy.
pub trait AgentFiles {
    fn config_exists(&self, agent: Agent) -> bool;
    fn skill(&mut self, agent: Agent, action: Action) -> Result<Outcome, IntegrationError>;
    fn mcp_executable(&self) -> Result<PathBuf, IntegrationError>;
    fn opencode(
        &mut self,
        action: Action,
        executable: Option<&Path>,
    ) -> Result<Outcome, IntegrationError>;
}
/// Resolves a binary without executing it.
pub trait PathLookup {
    fn resolve(&self, name: &str) -> Option<PathBuf>;
}
#[derive(Clone, Debug)]
pub struct CommandOutput {
    pub exit_status: Option<i32>,
    pub success: bool,
    pub output: String,
}
/// Arguments are passed as argv, never interpreted by a shell.
pub trait CommandRunner {
    fn run(
        &mut self,
        agent: Agent,
        program: &Path,
        args: &[OsString],
    ) -> Result<CommandOutput, IntegrationError>;
}
