# Ghostlink Public Release (zero-config install, token broker, distribution)

## Objective and scope
Make Ghostlink usable by anyone without registering their own Bungie application:
install with one command, run `ghostlink` (first-run wizard), sign in once, done
for ~90 days. One login serves both the CLI and `destiny-mcp`.

Target flow:

```
curl|sh (Linux) or irm|iex (Windows)
  -> ghostlink            first run launches the setup wizard
       detect Claude Code / Codex / OpenCode / Pi, multi-select with space
       install embedded skill + destiny-mcp config for the selected agents
  -> ghostlink login      browser authorization; Linux returns via ghostlink://,
                          Windows uses paste-the-URL for this release
  -> manifest auto-sync -> ready
```

Isolation: worktree `../ghostlink-release` on `feature/ghostlink-public-release`
from `origin/main` `dcaec71`. The main checkout's `target/debug/ghostlink` is the
installed desktop handler and must not be rebuilt.

## Decisions (user-approved 2026-10-05)
- **Credentials:** the owner's Bungie API key and public client id are embedded
  in the build (overridable by env and `config.env`). The client secret is never
  distributed.
- **Token broker:** a small owner-hosted, stateless service (Cloudflare Workers)
  holds the client secret and performs only the authorization-code exchange and
  the refresh grant. It stores nothing and never sees Destiny API traffic.
- **Distribution:** cargo-dist with GitHub Releases plus the shell (`curl`) and
  PowerShell (`irm`) installers only. No AUR, winget, MSI or code signing yet.
- **Wizard:** supports only Claude Code, Codex, OpenCode and Pi. Detects installed
  agents, vertical multi-select (space to toggle, enter to confirm), installs the
  skill embedded in the binary (`include_str!`, version-matched; no Node/npx) and
  the `destiny-mcp` server config. Merges into existing agent config without
  clobbering, backs up before writing, idempotent. Never prompts without a TTY or
  with `--json`.
- **Agents can use either path:** CLI skill or MCP server, documented separately
  in the README with each agent's own `mcp add` command.
- **Windows URI handler:** out of scope for this release; paste-the-URL login on
  Windows. Developed right after this feature, from the user's Windows machine.

## Agent targets (verified from local `--help` / docs, 2026-10-05)
| Agent | Detect | Skill dir | MCP |
|---|---|---|---|
| Claude Code | `claude` on PATH or `~/.claude/` | `~/.claude/skills/` | `~/.claude.json` `mcpServers` / `claude mcp add -s user` |
| Codex | `codex` on PATH or `~/.codex/` | `~/.codex/skills/` | `~/.codex/config.toml` `[mcp_servers.*]` / `codex mcp add` |
| OpenCode | `opencode` on PATH or `~/.config/opencode/` | `~/.config/opencode/skills/` | `~/.config/opencode/opencode.json` `mcp` (type local, command array) |
| Pi | `pi` on PATH or `~/.pi/` | `~/.agents/skills/` | `~/.pi/agent/mcp.json` `mcpServers` / `pi mcp add` |

## Readiness audit (2026-10-05, read-only explorer)
- Keyring shared by CLI login, CLI reads and MCP (`credential_store.rs:50-56`).
- Read runtime requires `BUNGIE_API_KEY`, `DESTINY_DATABASE_PATH`,
  `DESTINY_DEFINITION_VERSION`, `DESTINY_FRESHNESS_TTL_SECONDS`
  (`destiny-app/src/config.rs`); a missing `config.env` fails every read.
- Token endpoint is a const (`destiny-adapters/src/token.rs:36`); refresh requires
  a secret; three composition points (CLI login, CLI read runtime, MCP).
- No DB path default, no parent `create_dir_all` (`sqlite/mod.rs:75-76`).
- No manifest auto-sync; definition version label is required.
- `config.env` path ignores Windows (`config/file.rs:15-27`, no `HOME`).
- No setup command; bare `ghostlink` prints help (clap required subcommand).
- No cargo-dist, no release workflow, no package metadata; Linux needs libdbus.

Open uncertainties: definition-label mismatch semantics in persisted snapshots;
cargo-dist handling of the libdbus runtime dependency.

## Constraints and acceptance
- Test-first for every behavior task: observe RED, then GREEN.
- Checks per task: `cargo fmt --all -- --check`, `cargo build --workspace`,
  `cargo test --workspace --all-features`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `git diff --check`.
- The agent never reads or writes secrets, never runs `ghostlink login`, never
  edits the user's real agent configs or handler association. Live validation is
  human-run with sanitized output.
- Delivery: work-unit commits on `feature/ghostlink-public-release`, layered so
  the work can ship as chained PRs (broker / zero-config / wizard / distribution).
  Push, PR and merge are user decisions.

## Tasks

### Phase 0
- [x] **PR-001 — Establish tracker and branch.** Tracker plus README "Use it from
  an AI agent" rewrite (Option A MCP per-agent commands, Option B CLI skill;
  one shared login). Docs only (no meaningful RED); `git diff --check`. Commit evidence: `e491aa6`.

### Phase 1 — Token broker
- [ ] **PR-002 — Broker service.** Stateless Cloudflare Worker: `POST /token`
  (code exchange) and `POST /refresh`, secret from Worker secrets, Basic auth to
  Bungie, strict input validation, rate limiting, no logging of codes or tokens.
  Open decision: language (TypeScript vs Rust/WASM) and location (`broker/` in
  this repo vs separate repo).
- [ ] **PR-003 — Broker mode in the token adapter.** Configurable token endpoint
  (compiled-in broker URL, overridable); broker mode sends no Basic header and no
  secret; refresh available without a secret; wire CLI login, CLI read runtime
  and MCP. Direct confidential mode stays for development.

### Phase 2 — Zero configuration and wizard
- [ ] **PR-004 — Compiled-in defaults layer.** Precedence env > `config.env` >
  compiled defaults for API key, client id, callbacks, broker URL, definition
  label and freshness TTL. Open decision: commit the (non-secret) API key and
  client id in the repo vs inject them only in release CI.
- [ ] **PR-005 — OS data and config paths.** Default DB at
  `<data_dir>/ghostlink/ghostlink.db` (`$XDG_DATA_HOME`/`~/.local/share`,
  `%LOCALAPPDATA%`) with `create_dir_all`; `config.env` under `%APPDATA%` on
  Windows.
- [ ] **PR-006 — Manifest auto-sync.** Sync definitions automatically after login
  and on first read when none are loaded; resolve the definition-label question.
- [ ] **PR-007 — Agent integration writers.** Detection plus skill install and
  MCP config merge for Claude Code, Codex, OpenCode and Pi (backup, idempotent,
  no clobbering); `ghostlink agents {status,install,uninstall}`-style surface.
- [ ] **PR-008 — First-run setup wizard.** `ghostlink setup` and bare `ghostlink`
  on first run: multi-select of detected agents, install, then login and manifest
  sync; non-interactive fallback.

### Phase 3 — Distribution
- [ ] **PR-009 — cargo-dist release.** Package metadata, `ghostlink` +
  `destiny-mcp` artifacts (exclude `offline-fixture`), shell and PowerShell
  installers, libdbus handling, release workflow on tags, self-update.
- [ ] **PR-010 — Docs refresh.** README quick start for end users, contributor
  build path, `docs/configuration.md`, skill notes (wizard, prefer MCP tools when
  present).
- [ ] **PR-011 — Live validation.** Owner deploys the broker; human-run install
  from a release on Linux and Windows (paste login), wizard, MCP and CLI reads,
  post-expiry refresh through the broker; close the tracker.

## Follow-ups (out of scope)
- Windows `ghostlink://` handler (next feature, developed on Windows).
- winget / AUR / MSI / code signing.
