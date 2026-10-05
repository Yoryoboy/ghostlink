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
