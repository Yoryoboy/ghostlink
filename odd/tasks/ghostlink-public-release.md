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
- [x] **PR-002 — Broker service.** TypeScript Cloudflare Worker in `broker/`
  (decided 2026-10-05). Single strict `POST /token` proxy for both grants:
  accepts exactly the confidential-shaped bodies (`authorization_code`: code +
  redirect_uri; `refresh_token`: refresh_token), rejects extra/duplicate/empty
  fields, pins `redirect_uri` to `ALLOWED_REDIRECT_URI`, adds Basic auth
  (form-encoded id/secret, same as Rust), no redirects, 10 s deadline, 4 KiB
  request / 8 KiB JSON-only relay, fixed OAuth-style errors, `no-store`, rate
  limit binding per IP (fails closed on binding error), observability off, no
  logging. CI job `broker` (typecheck + tests). Delegated writer, test-first
  (RED: 67 failed / 1 passed on 501 stub; GREEN: 68, then 72 after
  triangulation). Parent re-ran: 72 passed, typecheck clean, diff check. Not
  deployed (owner, PR-011). Commit evidence: `edbcf2b`.
- [x] **PR-003 — Broker mode in the token adapter.** Optional
  `GHOSTLINK_TOKEN_BROKER_URL` (https only, no userinfo/query/fragment, no
  controls, <= 2048 bytes; endpoint `<base>/token`, trailing slash and path
  prefix handled). Explicit `ClientAuthMode::{Public, Confidential, Broker}`,
  precedence secret > broker > public, in config and all three roots (CLI login,
  CLI read runtime, MCP). `BungieTokenExchange::brokered(..)` and
  `HttpTokenTransport::brokered(base)`: no Authorization header, no client
  fields, refresh with rotation, 400/401 -> reauthorization, 403 unchanged.
  Existing public/confidential bytes and outputs unchanged. Compiled-in broker
  URL moved to PR-004. Delegated writer, test-first (RED: broker config did not
  enable refresh, then missing-API compile errors; GREEN). Verifier FAIL on one
  source-traced finding: `https:////@host` bypassed the empty-userinfo check
  because `url` drops empty userinfo. Parent fix, test-first (RED: both
  validation tests failed on the new cases; GREEN): require the literal
  `https://` prefix and a non-empty authority without `@`. Note: request and
  transport Debug show the broker URL (not secret). Checks: fmt, clippy,
  1350 passed / 0 failed, diff check. Commit evidence: `94d7435`.

### Phase 2 — Zero configuration and wizard
- [x] **PR-004 — Compiled-in defaults layer.** Decision (user, 2026-10-05):
  commit the official PUBLIC values in the repo (extractable from binaries
  anyway; the client secret never). `crates/destiny-app/src/defaults.rs`: API
  key, client id and broker URL are `None` until the owner fills them with
  `Some(..)`; callbacks default to `ghostlink://oauth/callback`, TTL to 900.
  Precedence env > `config.env` > compiled, in one place
  (`exact_source_values_with_defaults`, injectable `CompiledDefaults`); existing
  validators apply to defaults; empty/invalid overrides never fall back; secret
  still beats a compiled broker URL. `DESTINY_DEFINITION_VERSION` NOT defaulted:
  it is persisted in profile sync metadata (`sqlite/inventory.rs:889-905`) and
  part of snapshot equality (`destiny-core/src/ports.rs:158`), so it moves to
  PR-006. Delegated writer, test-first (RED: missing injection API, then 2
  failing default/precedence tests; GREEN). Parent re-ran: fmt, clippy, 1355
  passed / 0 failed, diff check. Commit evidence: `cee6617`.
  Owner action pending: fill API key, client id and (after deploy) broker URL.
- [x] **PR-005 — OS data and config paths.** `DESTINY_DATABASE_PATH` optional:
  env > `config.env` > OS default (`$XDG_DATA_HOME` absolute or
  `~/.local/share` + `ghostlink/ghostlink.db`; Windows `%LOCALAPPDATA%\ghostlink\
  ghostlink.db`); unresolvable keeps the sanitized missing-config error.
  Dependency-free resolver `config/paths.rs` with injected `Platform` so both
  OS rules are tested on any OS. Windows `config.env` at
  `%APPDATA%\ghostlink\config.env` (`GHOSTLINK_CONFIG_FILE` still wins; Unix
  resolution unchanged). `SqliteStore::open` creates missing parents (Unix
  0700 for new dirs, existing perms untouched, sanitized storage error); covers
  CLI reads, sync, sync-manifest and MCP. Delegated writer, test-first (RED: 4
  intended failures; GREEN). Windows rules verified via injection only, not on
  native Windows. Parent re-ran: fmt, clippy, 1361 passed / 0 failed, diff
  check. Commit evidence: `ae7d777`.
- [x] **PR-006 — Manifest auto-sync.** `DESTINY_DEFINITION_VERSION` optional:
  env > file override, else the version of the manifest stored in SQLite. CLI:
  when definitions are missing, runs the existing manifest sync once (stderr
  notice only, never in `--json` stdout); after `ghostlink login`, syncs if none
  stored (failure is non-fatal with a hint). MCP never downloads inside a tool
  call: lazy local resolution, typed missing-catalog error pointing to
  `ghostlink sync-manifest`. No schema migration: regression test shows an
  old-label snapshot stays readable, the switch to the stored version advances
  the revision exactly once, identical re-syncs do not (in-memory SQLite).
  Delegated writer; test-first only partially evidenced (RED observed for the
  config test; other behaviors went GREEN with compile-error intermediates).
  Writer reported an intermediate run reached the production manifest sync in
  a real-CLI test. Verifier FAIL (static): `hardening_leak_journey.rs`
  real-CLI test used a missing parent dir as "unopenable" DB, which PR-005's
  `create_dir_all` now creates; with a stored native-keyring credential
  (Windows/macOS dev machines) it could auto-sync over the network. Parent fix:
  the parent is now a regular file, so the open always fails before any read.
  Accepted note: an expired stored credential can let the CLI download the
  (public) manifest before the auth rejection; harmless and needed after
  re-login. Parent checks: fmt, clippy, diff check, and the full suite inside
  `unshare -rn` (loopback only, no outbound connectivity proven with curl):
  1367 passed / 0 failed. Commit evidence: `0028a85`.
- [x] **PR-007 — Agent integration writers.** `ghostlink agents
  {status,install,uninstall} --agent <claude|codex|opencode|pi>... [--skill]
  [--mcp]` (both by default), `--json` supported, exit 51 if any step failed,
  manual-step outcomes exit 0. Detection: binary on PATH (never executed) or
  config dir. Skill embedded via `include_str!`, written to each agent's skills
  dir; uninstall removes only our `ghostlink` skill. MCP registers the absolute
  sibling `destiny-mcp` path (typed error if missing): Claude/Codex/Pi through
  their own `mcp add/remove` CLIs (argv only, bounded output, timeout); OpenCode
  merges `opencode.json` `mcp.ghostlink` with per-run backup and atomic write,
  and leaves JSONC/invalid files untouched with a manual snippet. Layering
  (parent review): ports and shared types in `destiny-core::agents`,
  orchestration in `destiny-app::agents`, adapters depend only on core
  (`cargo tree` confirms no production destiny-app edge). Delegated writer,
  test-first for engine behaviors (RED: 7 failing behavior tests + 1 status
  assertion); CLI JSON/parsing tests GREEN-only. No real agent binaries or user
  configs touched (fakes, worktree-local fixtures). Parent checks: fmt, clippy,
  diff check, full suite in `unshare -rn`: 1378 passed / 0 failed. Live agent
  and native Windows behavior pending PR-011. Commit evidence: `4b5b44c`.
- [x] **PR-008 — First-run setup wizard.** `ghostlink setup` (always) and
  bare `ghostlink` (TTY and no marker; otherwise help unchanged). Flow: detected
  agents only, all preselected, `dialoguer` MultiSelect (space/enter); install
  mode Select (skill + MCP recommended / MCP only / skill only) through the
  PR-007 engine with per-agent outcomes and OpenCode manual snippet; Confirm
  login (existing callback-auto flow, manifest sync after login). Esc/Ctrl+C
  abort cleanly (non-zero, no marker); Unix prompt-scoped SIGINT no-op handler
  so dialoguer returns Interrupted and restores the cursor. Marker
  `<data_dir>/ghostlink/setup-complete` written only on reaching the end.
  Non-TTY or `--json` `setup` -> typed usage error naming the non-interactive
  equivalents. Dependency `dialoguer 0.12` (MIT, default features off), fetched
  by the parent after the writer blocked on offline resolution. Parent review
  fix: the writer had included `config/paths.rs` twice via `#[path]` (+
  `clippy::duplicate_mod`); now `config::paths` is `pub(crate)` and reused.
  Delegated writer, test-first (RED: 3 scripted wizard tests on stub; GREEN);
  routing/negative tests GREEN-only; real dialoguer cancellation covered in
  isolated PTY tests (Unix). Parent checks: fmt, clippy, diff check, full suite
  in `unshare -rn`: 1390 passed / 0 failed; non-TTY bare run prints help and
  writes nothing. Native Windows console cancellation unverified. Commit
  evidence: `4ece4e5`.
  User feedback (2026-10-05): closing line is just `Done.` (no agent tip);
  test-first (RED: completion test failed; GREEN). Commit evidence: `367976d`.

### Phase 3 — Distribution
- [x] **PR-009 — cargo-dist release.** cargo-dist 0.32.0 (`dist-workspace.toml`):
  targets `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, `shell` and
  `powershell` installers, `install-path = "CARGO_HOME"`, `install-updater`
  (`ghostlink-update`), `precise-builds`, apt `libdbus-1-dev` + `pkg-config`.
  One archive and installer per platform with BOTH `ghostlink` and
  `destiny-mcp` in the same directory (required by PR-007 sibling resolution).
  Package `destiny-cli` renamed to `ghostlink` (library stays `destiny_cli`, no
  import changes) and owns both bins; MCP entry extracted to
  `destiny_mcp::run()`; `destiny-mcp` package is `dist = false` (library +
  test-only `offline-fixture`). libdbus kept dynamic (`libdbus-1.so.3`,
  documented); vendoring evaluated and rejected (keeps system security
  updates). Tag-triggered `.github/workflows/release.yml` from `dist generate`;
  CI build command updated; workspace metadata (repository, homepage,
  description, authors). Delegated writer (packaging: no meaningful RED);
  local `dist build` for Linux: archive contains both executables,
  `ghostlink --version` and `destiny-mcp --help` ran from the extracted archive.
  Parent review fix: removed the duplicate standalone `destiny-mcp` bin in the
  `destiny-mcp` package (Cargo output-filename collision warning, may become a
  hard error); docs use `cargo run -p ghostlink --bin destiny-mcp`. Parent
  checks: no collision, fmt, clippy, `dist plan`, `dist generate --check`,
  diff check, full suite in `unshare -rn`: 1390 passed / 0 failed. Installers
  and native Windows build not executed (first real tag in PR-011). Commit
  evidence: `b060874`.
- [x] **PR-010 — Docs refresh.** README for end users: Install (cargo-dist
  `curl`/`irm` one-liners from `releases/latest/download`, both executables
  side by side, `ghostlink-update`) -> Set up (wizard, one shared login,
  ~90-day renewal, Linux handler vs Windows paste) -> Use it -> manual agent
  integration (`ghostlink agents ...`, per-agent MCP commands, skill) ->
  privacy -> For developers. Provisioning caveat kept until PR-011.
  `docs/configuration.md`, `cli.md`, `mcp.md`, `development.md` aligned (config
  optional, precedence, three OAuth modes, OS paths, automatic definition
  version, `-p ghostlink`). Skill prefers Ghostlink MCP tools when present,
  first-run guidance, automatic manifest. Delegated writer (passive docs: no
  meaningful RED); checks: `skill_drift` 5/5, README commands checked against
  `--help`, diff check. Commit evidence: `861d9aa`.
- [ ] **PR-011 — Live validation.** Owner deploys the broker; human-run install
  from a release on Linux and Windows (paste login), wizard, MCP and CLI reads,
  post-expiry refresh through the broker; close the tracker.
  Live log (2026-10-05, human-run where interactive):
  - Broker deployed with wrangler 4.147 to
    `https://ghostlink-token-broker.93jads.workers.dev` (`BUNGIE_CLIENT_ID`
    55099 var; `BUNGIE_CLIENT_SECRET` set by the owner via `wrangler secret
    put`, never seen by the agent). Smoke: GET 405, non-form POST 415, other
    path 404, form POST before the secret 500 `server_misconfigured`.
  - `defaults.rs` filled (API key, client id 55099, broker URL); fmt, clippy,
    1390 passed / 0 failed in `unshare -rn`. Commit evidence: `cf39e7d`.
  - Broker-mode login with compiled values only
    (`GHOSTLINK_CONFIG_FILE=/nonexistent ghostlink login --callback handler`):
    `authorized`, renewable session until ~90 days; manifest auto-synced after
    login. Note: `--callback paste` cannot work on a machine where another
    binary owns `ghostlink://` (the browser hands the URL to that handler).
  - `list-characters` with compiled values only: 3 characters, fresh; default
    DB created at `~/.local/share/ghostlink/ghostlink.db`.
  - Pending: post-expiry refresh through the broker (access expired after
    ~1 h), push/PR/merge, tag pre-release, installer tests on Linux and
    Windows, wizard and MCP reads from installed binaries.

## Follow-ups (out of scope)
- Windows `ghostlink://` handler (next feature, developed on Windows).
- winget / AUR / MSI / code signing.
