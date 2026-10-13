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
  - Integration: merged `origin/main` (#47/#48 perk descriptions) into the
    branch without conflicts; 1411 passed (all features) and 1396 (default)
    in `unshare -rn`; broker 72 tests; `dist plan` unchanged.
  - Windows CI found a real bug: the skill ownership check required an LF
    `---\n` prefix, but Windows checkouts (autocrlf) embed CRLF through
    `include_str!`, so reinstalling reported a foreign skill. Fix, test-first
    (RED: two new CRLF tests failed on Linux; GREEN): normalize CRLF in the
    check, plus `.gitattributes` `skills/** text eol=lf` so every platform
    embeds identical bytes; tests build paths with `join`. Commits `203d3f8`,
    `222f8b9` (format follow-up: a `;`-chained fmt check had let an unformatted
    commit through). CI green on ubuntu, windows and broker.
  - PR #50 merged into `main` as `43786a6`.
  - Pending (release session): post-expiry refresh through the broker; bump to
    `0.1.0-rc.1`, tag `v0.1.0-rc.1` (cargo-dist marks it pre-release), test
    installers on Linux and Windows via versioned URLs, wizard and MCP reads
    from installed binaries, then `0.1.0` stable. On hold while the owner
    finishes parallel work to include in the release.
  - Pending (release session), lifecycle commands (see
    `odd/tasks/ghostlink-lifecycle.md` T7), in a disposable Linux container or
    VM, never on the owner's machine: install `rc.1` with the official
    installer; `ghostlink uninstall --dry-run` (no writes), cancelled
    confirmation, then `ghostlink uninstall --yes` removing only receipt-owned
    files while config, database and credentials remain; reinstall. Update
    needs two releases: install `rc.1`, publish `rc.2`, run `ghostlink update`
    and `ghostlink update --version`, then restart MCP and check the installed
    version. Windows: confirm both commands refuse with manual guidance.
  Live log (2026-10-08, `v0.1.0-rc.1`, Linux): repo made public temporarily
  (installer assets 404 while private); owner installed via the versioned
  `installer.sh` into `~/.cargo/bin`, login authorized, manifest auto-synced.
  CLI reads on the Hunter all exit 0 and fresh: list-characters, loadouts,
  postmaster, subclass, artifact, weekly, weekly-reset, search-inventory.
  Findings to file as one issue after the live test:
  - F1 (blocker) MCP clients reject `tools/list`: `inspect_item` and
    `move_item` output schemas are top-level `oneOf` without `type: object`
    (`crates/destiny-mcp/src/schema.rs`); Pi fails to connect at all.
  - F2 `search-inventory --character hunter` silently returns 0 hits; the
    flag accepts only an exact ID while other commands accept a class.
  - F3 `weekly-reset` prints `unresolved #1783825372` modifiers.
  - F4 README install uses `releases/latest`, which 404s while only
    prereleases exist.
  - F5 README does not say a new shell is needed after install
    (`~/.cargo/bin` PATH).
  Filed as #66; uninstall abort on an active `ghostlink://` handler filed as #67.
  Live log (2026-10-08, `v0.1.0-rc.2`, Linux): rc.1 removed manually, rc.2
  installed with the versioned `installer.sh`. Pi MCP connects (17 tools;
  #66 F1 fixed). F2 fixed (class accepted, invalid value errors), F3 fixed
  (0 unresolved modifiers), F4/F5 README fixed. MCP reads OK: list_characters,
  get_postmaster, inspect_item.
  New findings:
  - G1 MCP `get_loadouts` for one character returns ~110 KB JSON (~250 KB
    with text + structuredContent), 4 loadouts; Pi truncates it. Owner's
    direction (separate work): `get_loadouts` lists summaries only (index,
    name, subclass) and a per-loadout call returns one loadout's detail.
    Also review the duplicate payload: every tool returns the same JSON as
    text content and as `structuredContent`, doubling context cost.
  - G2 Back-to-back moves read a stale profile: CLI move to Vault (applied),
    MCP move back to Hunter (applied, confirmed in game), then a CLI move's
    fresh preflight and a forced `sync` still showed the item in the Vault,
    so the planned Vault -> Hunter step ended `uncertain` (exit 41).
    Root cause (confirmed later): mostly our local projection, not Bungie.
    After an MCP `applied` move, `inspect-item` and the CLI move preflight
    still saw the old location for 90+ s (CLI refused: "already in Vault",
    exit 40), yet one explicit `ghostlink sync` showed the new location at
    once. Optimistic actions neither apply their effect to the shared SQLite
    projection nor invalidate it. Owner wants chained actions fast: apply
    acknowledged effects locally (DIM-style), or at least invalidate so the
    next read/preflight forces a sync. Bungie also lags: after a CLI move to
    Vault (confirmed in game), forced syncs showed the old location for
    ~20-30 s. So invalidation alone is not enough; chained actions need the
    acknowledged effect applied locally. Owner priority: HIGH, actions must
    feel as fast as possible; no cache-induced waits between chained actions.
  - G3 (correctness) False `verified_noop`: MCP `set_lock_state` unlock
    returned `applied` (confirmed in game); an immediate lock call returned
    `status: verified_noop`, `observed: {locked: true}` from the stale
    projection, while the item was actually unlocked. A "verified" claim
    must never come from data older than our own acknowledged action. Both
    responses also said `profile_refresh: not_required`, contradicting the
    tool description ("local freshness is invalidated").
  Observed Bungie read lag after acknowledged actions (forced sync, item
  confirmed in game): moves ~20-30 s; lock/unlock <10 s to ~70 s; Vault item
  lock ~120 s. Move and lock/unlock validated live via MCP and CLI on a
  character item and a Vault item (MIDA Mini-Tool: MCP lock, CLI unlock).
  Extra G2 evidence: right after the Vault unlock, consecutive forced syncs
  flapped locked true/false/true (inconsistent Bungie API cache nodes) before
  settling. (A suspected CLI unlock failure was a misread item in game, not
  a defect.)
  - G4 Postmaster pulls (MCP and CLI) refused with an opaque
    `definition_version_mismatch` because the owner's old dev `config.env`
    set `DESTINY_DEFINITION_VERSION=live-v1`, which stamps profile snapshots
    while definitions carry the real Manifest version. The refusal should
    name both versions and point at the override; consider warning on (or
    ignoring) a label override that differs from the stored Manifest.
    Owner commented the line out (backup `config.env.bak`); `destiny-mcp`
    needed a restart to pick it up (config is read once at startup).
    Note: this machine's config also sets `BUNGIE_CLIENT_SECRET` and
    callbacks, so it is not a fresh-user (broker) setup.
  - G5 CLI `pull` text output prints raw JSON fields (`Plan: null`,
    `Failure: {"capacity_reason":...}`) instead of a human sentence such as
    "Power weapons slot is full (10/10); free a slot and retry". The
    capacity refusal itself is correct (planning stage, no dispatch).
  Postmaster pull validated live: MCP `pull_item` Frozen Orbit applied and
  arrived in game.
  G2 impact on capacity safety: right after that pull filled the Hunter's
  energy slot (10/10), Bungie still reported Frozen Orbit in the Postmaster
  and 9 energy weapons, so the CLI pull of Trust (energy) passed the
  capacity proof on stale data, dispatched, and ended `uncertain` (exit 41).
  Applying acknowledged effects locally would have refused it at planning.
  After moving Frozen Orbit to the Vault, Bungie reads flapped
  character/postmaster/character for ~128 s before showing the Vault; then
  CLI `pull` of Trust applied (exit 0).
  - G6 (blocker) `pull_stack` / `pull-stack` always refuse ordinary stacks:
    Enhancement Core (3853748946) refused at planning with
    `capacity_reason: unknown_uniqueness` on MCP and CLI (exit 40). Bungie's
    live definition has `stackUniqueLabel: null` (`maxStackSize` 9999);
    `crates/destiny-app/src/stack_pull.rs:352` maps an absent label to
    `None` (unknown) instead of `Some(false)` (not unique). Most items have
    no label, so non-instanced pulls are effectively unusable.
  - G7 `pull-stack` CLI: `--character` requires an exact ID (class not
    accepted, unlike other commands since #66 F2); an `--item-hash` flag is
    rejected with "argument is not valid here" instead of hinting that the
    hash is positional. Text output also prints raw JSON (same as G5).
  Equip: MCP `equip_item` correctly refused two exotics while Wavesplitter
  (exotic) was equipped. Equipping legendary Rose first refused with "local
  state changed after the plan was created" (nothing ran; likely G2 read
  flapping between plan and execution), then applied on one retry. CLI
  `equip` Better Devils applied. Loadouts: MCP `apply_loadout` PvP (by
  name, slot 3) and CLI `apply-loadout` Stasis (slot 1) applied, confirmed
  in game. CLI takes the loadout positionally while MCP uses `name`/`slot`
  (G7 consistency). `equip_items` (multi) not exercised.
  G1-G7 filed as #71. Repo made private again.
  Uninstall on the owner's machine (rc.2, #67 recheck): `--dry-run` now warns
  that handler skips will end partial (exit 41); `--yes` removed the three
  binaries, the receipt and the `~/.agents` skill, exit 41 `partial` with
  step-by-step `xdg-mime` follow-ups. Preserved: handler `.desktop`,
  `previous-uri-handler`, config, DB, `~/.cargo/bin/.ghostlink-lifecycle.lock`,
  and the Pi `mcp.json` entry (manual follow-up). Report still goes to
  stderr only. Still pending: broker token refresh, `equip_items`,
  Windows, update across two releases. Same lag made an MCP `move_item` to Vault fail
    with a generic `unsupported_operation` (stale "already in Vault"); the
    error should say the item is already at the destination.

  Live checklist (`v0.1.0-rc.3`, pending, human-run):
  Record sanitized outcomes, CLI exits/MCP errors, `local_state`, and in-game
  confirmation in the next live log; do not record credentials or live IDs.
  - [ ] Chain moves, equips and locks with no waits or forced sync between
    actions (including CLI/MCP alternation): move then move back, equip then
    another equip, unlock then lock. Confirm no stale refusal or false no-op.
  - [ ] Pull Postmaster equipment instances until the owner's destination bucket
    is full, then confirm the next pull refuses locally without dispatch.
  - [ ] Pull an ordinary Enhancement Core stack with an explicit amount and owner;
    confirm local Postmaster/inventory quantities and in-game receipt.
  - [ ] In a controlled stale/unknown-occupancy case where a full-destination
    Postmaster pull reaches Bungie, confirm numeric code 1642 and typed
    `destination_full`, or record the actual code/reason returned. Do not bypass
    a known-full local refusal just to obtain the code.
  - [ ] Apply a saved loadout including a subclass switch, then immediately equip
    an item without waiting; confirm local equipment/subclass and game state.
  - [ ] Measure `get_loadouts` response size in Pi and confirm summaries are not
    truncated; fetch `get_loadout` detail by both saved name and slot.
  - [ ] After Ghostlink caches an item, move it in game behind Ghostlink's back;
    attempt the stale action and record typed `item_not_found` and its actionable
    message (numeric 1623), or the actual rejection if Bungie returns another code.
  - [ ] Confirm `local_state`: `updated` after complete acknowledged effects,
    `unchanged` for local no-op/pre-dispatch refusal, `refresh_pending` after
    rejection/uncertainty or incomplete effects. Confirm the next ordinary read
    syncs only when missing/stale/invalidated; never blindly retry a failure.
  - [ ] Pending from rc.2: post-expiry broker token refresh with broker defaults.
  - [ ] Pending from rc.2: `equip_items` multi-item batch, including local equipment
    effects and an immediate follow-up action.
  - [ ] Pending from rc.2: native Windows install, paste login, wizard, MCP/CLI
    reads and documented lifecycle refusal/manual guidance.
  - [ ] Pending from rc.2: update across two releases in a disposable Linux
    container/VM, check installed versions, restart MCP and verify reads; check
    Windows manual updater guidance as described in the lifecycle log above.

  Live checklist (next RC — DIM parity, pending, human-run):
  Keep the rc.3 log above as historical evidence. Record sanitized outcomes,
  CLI exits/MCP errors and local projection state; never credentials or live IDs.
  - [ ] Apply a saved loadout with one item in the Vault and one equipped on
    another character. Compare the receipt, local projection and in-game result.
    Owner decision: if EquipLoadout does not pull them, decide whether a follow-up
    DIM-style preparation pipeline is needed; current behavior does not promise it.
  - [ ] Observe throttle fail-fast (`retry in about N seconds`) and a successful
    caller-driven read after cooldown. Confirm no action redispatch; do not flood
    Bungie to manufacture throttling.
  - [ ] Exercise CLI positional queries (including leading `-`) and MCP `query`:
    AND with flags, parentheses, phrases/exact names, numeric comparisons and
    explicit-and/or/whitespace precedence. Confirm empty query searches all owned.
  - [ ] Check invalid queries in CLI (exit 2, kind/byte span/excerpt) and MCP
    (`invalid_query`, kind/span/message; supported_filters for unknown keys).
    Include unsupported DIM filters; no silent relaxation.
  - [ ] Compare compact/default and detail search hits, referenced stats and
    owned pagination (offset/next_offset), with unchanged definition candidates.
  - [ ] Inspect by exact normalized name in CLI/MCP, including ambiguity with
    compact candidates and no fuzzy selection; choose a genuine instance ID.
  - [ ] Run concurrent fresh MCP reads while a mutation runs; confirm reads are
    not globally serialized. Trigger stale reads together and confirm sync is
    single-flight/exclusive with mutations.
  - [ ] Confirm apply_loadout's typed local_state status, unresolved_items,
    skipped and persisted, nullable item metadata and reason codes; exits 0/41/42
    retain their meanings. Describe effects as projected locally, not per-item
    Bungie confirmations.
  - [ ] Start a Claude client session and accept the full MCP tool list. Confirm
    apply_loadout slot/name and inspect_item id/name exclusivity is enforced at
    runtime, with no top-level oneOf/anyOf/allOf in advertised input schemas
    (the rc.3 apply_loadout schema bug is fixed).

  Live log (2026-10-13, `main` a86584e, local release build, human-run pending):
  Owner decision (2026-10-13): do not use the release path for this validation;
  run the pending DIM-parity checklist directly from `main`. The unmerged PR #84
  (`release/v0.1.0-rc.4`) differs from `main` only by the workspace version
  string, the README prerelease installer URL and one `.engram/` memory chunk
  (verified with `git diff main origin/release/v0.1.0-rc.4`) — no code. So `main`
  reports `0.1.0-rc.3` while carrying post-rc.3 code, and the checklist item that
  verifies the installed version says nothing in this run. Out of scope here
  because they need a published release: versioned installer URLs, `update`
  across two releases, `uninstall`/receipt evidence (#67) and native Windows.
  Build: `cargo build --release -p ghostlink` in `/home/yoryo/Work/ghostlink`
  (bins `ghostlink`, `destiny-mcp`); `destiny-mcp` installed to
  `~/.cargo/bin/destiny-mcp`, which is what `~/.pi/agent/mcp.json` already
  points at. Live checks therefore run the same binaries and the same default
  database a normal install uses.
  Database: the real projection at `~/.local/share/ghostlink/destiny.db`.
  Connecting the rebuilt MCP server opens that database and migrates it in
  place (schema 19 -> 22); `PRAGMA quick_check` returned `ok` and the rows were
  intact (3 characters, 697 items, 36,680 definitions). A recoverable copy of
  `destiny.db` plus its `-wal` was taken before any mutation
  (`~/.cache/ghostlink-main/`). An isolated copy was dropped on purpose: the
  CLI and MCP must share one projection for the CLI/MCP-alternation checks, and
  pointing the user's MCP entry at a scratch database would outlive the test.
  The other machine's memory chunk (`194fd5b8`, 36 observations, 177 sessions,
  93 prompts) was imported into this machine's Engram store the same day.
  Next-RC DIM-parity checklist, run 2026-10-13 from `main` a86584e (local
  release build, real account, real projection). 22 CLI cases, 8 MCP cases.
  - [x] CLI positional queries: empty query lists all owned; leading `-`
    (`-is:locked`); parentheses with `or`; whitespace/`and`/`or` precedence;
    AND with structured flags (`is:weapon --location vault`); quoted phrase
    (`type:"Hand Cannon"`); numeric comparison (`power:>=1800` -> 0 hits);
    plain NOT semantics (`-power:>2000` keeps low-power items, as decided).
  - [x] Definition candidates are unchanged: they do not respond to the query
    and stay bounded by `--limit`.
  - [x] Invalid queries: `is:bogus` -> exit 2 `invalid_filter_value`;
    `notes:godroll` -> exit 2 `unsupported_filter` (never relaxed);
    `(is:weapon` -> exit 2 `unbalanced_parenthesis`. MCP: `is:bogus` ->
    `invalid_query` + `kind` + `span` + `message`; `zzz:1` ->
    `unknown_filter` plus `supported_filters` (46 entries).
  - [x] Compact vs detail, pagination, inspect by name: compact tile by
    default; `--detail`/MCP `detail: true` keep the nested shape; CLI
    `--offset`/MCP `offset` return the same page and the same `next_offset`
    (`is:weapon -is:locked`, page 2 -> `next_offset: 4`); a unique exact name
    resolves case-insensitively and with surrounding whitespace (`sunshot`,
    `  Sunshot  `); an ambiguous name returns `ambiguous_match` (CLI exit 31,
    same code over MCP) with compact candidates; a partial name returns
    `item_not_found` (exit 32), so there is no fuzzy selection. Normalization is
    case/whitespace only, so `Sorrows Verse` correctly does not match
    `Sorrow's Verse` (`docs/cli.md:807`). `--filters` lists the registry with
    the syntax summary.
  - [x] `get_loadouts` payload: compact output is 2,040 bytes for all three
    characters (was ~110 KB for one character in rc.2) and arrived complete in
    Pi in both text content and `structuredContent`. Pi accepted all 18 tools
    and the server instructions, so the rc.3 top-level `oneOf` rejection does
    not reproduce (a real Claude client session is still pending).
  - FINDING (low, readability): malformed invalid-query diagnostics. Filed as #85. The CLI
    builds a three-line diagnostic — `crates/destiny-cli/src/command.rs:620-630`
    joins `error.message`, `\n` and `error.excerpt(input)`, and
    `crates/destiny-core/src/search_query.rs:140` puts the caret row on its own
    line — but the emitted message is flattened onto one line, so the span, the
    excerpt and the caret run together: `invalid filter value at bytes
    0..8is:bogus^^^^^^^^`, and `at bytes 0..13notes:godroll^^^^^^^^^^^^^`. Both
    the human line and the JSON `message` show it. `CliError` documents "the
    single sanitized line written for a human" (`command.rs:1397-1402`), so the
    multi-line intent and the single-line contract disagree and the caret row
    can never align under its excerpt. Not a correctness problem: exit 2,
    `kind` and the byte span are correct and machine-readable.
  - FINDING (low, contract consistency): the unknown `stat:` name is the one
    invalid-query class without a span. Filed as #85. `stat:zzz:>1` exits 2 with
    `{"code":"unknown_stat_name","message":"unknown stat name; use a name from
    the loaded Manifest stat definitions"}` — no `kind`, byte span or excerpt,
    unlike every parser/registry rejection. A client that renders invalid-query
    spans uniformly has to special-case it.
  - [x] Advertised MCP schemas, the deterministic half of the Claude item: the
    live `tools/list` from `destiny-mcp` publishes 18 tools whose input schemas
    have no top-level `oneOf`/`anyOf`/`allOf`. `apply_loadout` is `type: object`
    with only `character` required and `character`/`name`/`slot` properties, so
    slot-versus-name exclusivity stays runtime-enforced; `inspect_item` exposes
    `instance_id`/`name`. `inspect_item` and `move_item` keep a `oneOf` in their
    OUTPUT schema, each alongside the top-level `type: object` that #66 F1
    required, so there is no regression. A real Claude client session is still
    pending.
  - OBSERVATION (pre-existing G1 residual, not a regression): `get_loadout`
    detail for one loadout is 24,635 bytes (CLI) and Pi truncates the MCP result
    at its 50 KB text budget; the payload is duplicated in text content and
    `structuredContent` (~2x) and the bulk is the Clarity `plug_insights` prose,
    repeated for every plug. G1 is fixed for the summary call, not for the
    detail call.
  Mutation run (2026-10-13, same build, owner in orbit). Setup created through
  the MCP `equip_item`: Mercurial Overreach moved Hunter -> Vault -> Warlock and
  was equipped there (`local_state: "updated"`, `status: "applied"`, three
  completed steps, `observed: null`, reason "Bungie accepted the action; not
  re-read"). A forced `ghostlink sync` then confirmed Bungie agreed (revision 46:
  Ace of Spades in the Vault, Mercurial Overreach equipped on the Warlock).
  `ghostlink apply-loadout --character hunter --name "PvP"` took 0.43 s and exited
  42, and the game state changed as follows (read back from Bungie at revision
  48 with the CLI):
  - Applied: Ace of Spades moved out of the Vault and is equipped on the Hunter,
    so Bungie's `EquipLoadout` DOES pull an item from the Vault.
  - NOT applied: Mercurial Overreach, which was equipped on the Warlock, stayed
    there; the Hunter still has its previous energy weapon (Basso Ostinato). So
    `EquipLoadout` does NOT pull an item equipped on another character.
  - FINDING F3 (high, correctness): the tool reported `receipt: "applied"` with an
    empty `unresolved_items`, so the un-applied energy weapon is invisible in the
    receipt. No reason code covers "not on the target character", so the local
    projection cannot detect it either. U4b/U4c answered: implement DIM's
    preparation (source dequips and ordered transfers) before `EquipLoadout`
    for the cross-character case; the Vault case needs nothing.
  - FINDING F4 (medium, contract/UX): the single `skipped` entry was the seasonal
    artifact (`unsupported_bucket`), and that alone produced
    `outcome: "local_state_failed"`, exit 42, reason "local state could not be
    updated after the action", while `persisted: true` and the local effects were
    written. Every saved loadout carries the artifact (NPA Repulsion Regulator,
    Slayer Baron Apothecary Satchel and Tablet of Ruin on this account), so every
    `apply-loadout` exits 42 with a persistence-failure message.
    `docs/cli.md:788-801` defines `status: partial` (unresolved/skipped items)
    separately from `failed` (persistence or invalidation failed) and then maps
    both onto 42.
  Concurrency, partially verified: the four MCP reads fired after the mutation
  (the apply was too fast to overlap; they ran 9 s later) and all reported the
  same `checked_at` (1791906308) and the same revision (48) once freshness had
  been invalidated, which is consistent with one shared profile acquisition
  serving concurrent stale reads (single-flight). "Reads run while a mutation is
  in flight" is still unverified: the next attempt fires the mutation and the
  reads in the same batch, so a read reporting the pre-mutation revision proves
  it did not wait behind the mutation.
  Chained-action run (R1) and batch equip (R10), same build, no forced sync
  between actions: ten chained CLI actions left the account exactly where it
  started — move `applied`; unlock correct `verified_noop`; lock `applied`;
  equip `applied` with transfer+equip; equip of the already-equipped item
  correct `verified_noop`; a move of a just-equipped item returned `uncertain`
  (exit 41) and moved nothing, reproducing the known follow-up "move of an
  equipped item reports uncertain instead of refused"; then three consecutive
  real equips in the same slot, a move back to the Vault and an unlock, all
  `applied`. All three documented string `local_state` values appeared live:
  `updated` (applied), `unchanged` (verified_noop) and `refresh_pending`
  (uncertain), and both no-ops were true (the item was already unlocked, the
  other already equipped). MCP `equip_items` with two items returned
  `outcome: "applied"`, `local_state: "updated"` and `status: "success"` per
  item; an immediate CLI unlock/lock pair then resolved the locations that batch
  had written.
  FINDING F5 (low, output text): `ghostlink postmaster` prints "Stacks are
  anonymous observations, not stable pull selectors. No pull feature is
  exposed." although `ghostlink pull`, `ghostlink pull-stack` and the MCP
  `pull_item`/`pull_stack` tools exist. The sentence is hardcoded in
  `crates/destiny-cli/src/postmaster_output.rs:101` and pinned by
  `crates/destiny-cli/tests/postmaster_pipeline.rs:153`, so it needs a wording
  fix with its test.
  Postmaster is empty for all three characters (`ghostlink postmaster`:
  Instances none, Stacks none), so R3, R2 and R4 cannot run until the account has
  Postmaster contents. The owner reports seeing engrams in the game, so this is
  not settled: `ghostlink search-inventory 'type:"Engram"'` does return four
  Exotic Engrams and places them at `location.kind = character` on the Hunter,
  which is consistent with an empty Postmaster if those are the engrams the owner
  saw. The human output itself warns `profile_currencies_coverage: available
  (component 103 only, not Postmaster completeness)`, so an empty CLI Postmaster
  is not proof that the game's Postmaster is empty. Awaiting one in-game look.
  FINDING F6 (low/medium, surface): `ghostlink search-inventory 'is:weapon'
  --location postmaster` fails with `error: invalid --location: expected one of
  vault, character`, while the same command's query language accepts
  `is:inpostmaster` and advertises it among the 46 `supported_filters`, and the
  storage model already has `location_kind IN ('vault','character','postmaster',
  'other')`. A user following `--help` cannot filter the Postmaster with the
  structured flag even though the query form works.
  In-game confirmation of F3 (owner, 2026-10-13): the Warlock has Mercurial
  Overreach equipped and the Hunter does not, matching the tool reads.
  Stale-preflight run, the R7 equivalent (2026-10-13). The external writer was a
  second Ghostlink cache against a separate database
  (`DESTINY_DATABASE_PATH=~/.cache/ghostlink-main/destiny.db`), used only to
  change Bungie's state; every assertion was read from the real database and the
  CLI was the only interface, so no human action was needed.
  - The real cache believed MIDA Mini-Tool was equipped on the Hunter
    (revision 63, fresh). The external writer equipped Basso Ostinato and moved
    MIDA Mini-Tool to the Vault; both reported `applied`.
  - The real cache then ran `ghostlink move <MIDA> --character warlock` with its
    stale plan: exit 41, `status: incomplete`, `local_state: refresh_pending`,
    step 1 (Hunter -> Vault) `uncertain` with reason "provider outcome is
    uncertain; do not blindly retry", step 2 (Vault -> Warlock) `not_run`, 0 of 2
    steps completed, `verification: not_verified`.
  - FINDING F8 (medium, error typing): the checklist expects a typed
    `item_not_found` with an actionable message (numeric 1623) when a stale
    action reaches Bungie. What comes back is a generic `uncertain` with no code
    and no statement of what happened, so the user cannot tell whether the item
    moved. It has the same shape as the other known case (moving a genuinely
    equipped item), so two different causes produce the same uninformative
    outcome. Safe (no retry, freshness invalidated) but not diagnosable.
  - FINDING F9 (observation): after an action returns `local_state:
    refresh_pending` — documented at `docs/cli.md:1127` as "invalidated state
    requiring the next ordinary read to sync" — the next ordinary read reported
    `freshness: fresh` with an unchanged `checked_at` and served the locally
    projected state. Reproduction: read MIDA Mini-Tool in the Vault
    (`revision 64`, `checked_at 1791907627`); equip it on the Hunter (`applied`,
    two completed steps); move it to the Vault while equipped (`incomplete`,
    `refresh_pending`, one `uncertain` step, 0 of 1 completed); ordinary read
    immediately after (`revision 65`, `checked_at 1791907627` UNCHANGED,
    `freshness: fresh`). Contrast: the reads after the earlier `apply_loadout`
    DID advance `checked_at` (1791906308), so the two invalidation paths behave
    differently, or the invalidation the move path writes is not what the read
    path checks. Mechanism undetermined; recorded for the follow-up session.
  - State after these runs (owner said no restore needed): `ghostlink sync`
    confirmed revision 66, MIDA Mini-Tool equipped on the Hunter and Basso
    Ostinato in the Hunter's inventory.
  - STILL BLOCKED on the owner: throttle fail-fast and recovery, not observable
    without flooding Bungie; a real Claude client session accepting the full tool
    list (Pi, on the same API, accepted it); post-expiry broker token refresh,
    which needs an expired token.

  F3 step 1 + F4 fix (2026-10-13, branch `fix/apply-loadout-honest-local-state`,
  commit `6dff3ab`). Decision: report an item EquipLoadout cannot reach instead of
  projecting it; DIM-style cross-character preparation stays a separate step 2.
  - Change: a saved non-subclass item on another character is unresolved
    `on_other_character`, one in the Postmaster `in_postmaster`; neither is
    projected, the apply is `partial` and freshness is invalidated. The seasonal
    artifact skip (`unsupported_bucket`, bucket 1506418338) is listed but no
    longer makes an apply partial. A partial apply keeps exit 42 with the reason
    "some loadout items were not applied; see local_state"; "local state could
    not be updated after the action" is reserved for `failed`. The old test
    `acknowledged_cached_apply_updates_equipment_revision_and_preserves_plugs`
    pinned F3 as intended; its fixture now uses an item on the target.
  - Checks: fmt, clippy `-D warnings`, `cargo test --workspace --all-features`
    green; independent verify passed S1-S4; native review approved twice
    (workspace and committed range).
  - Live run on the second PC (release build of the branch, DB backed up to
    `~/.cache/ghostlink-f3f4-backup/`, outputs in `runs/`), owner checking DIM:
    - T1 (F3): Mercurial Overreach equipped on the Warlock; apply PvP on the
      Hunter -> exit 42, receipt `applied`, `partial`, unresolved Mercurial
      `on_other_character`, artifact in `skipped`, `persisted: true`. The next
      read synced; Mercurial stayed on the Warlock, MIDA Mini-Tool stayed
      equipped, Ace of Spades and Corrective Measure (Timelost) were equipped.
      Confirmed in DIM. PASS.
    - T2 (F4): owner moved Mercurial to the Vault in DIM; after a forced sync,
      apply PvP -> exit 0, `applied`, `updated`, only the artifact in `skipped`,
      freshness retained; a later forced sync matched the projection. PASS.
    - T3/T4 (full loadout): apply Stasis then PvP, both exit 0 `updated`. After
      each, Bungie showed the saved subclass, super, abilities, both aspects, all
      five fragments, the saved artifact and its seven perks; after PvP, all three
      weapons and all five armor pieces equipped. Bungie's EquipLoadout switches
      subclass configuration and artifact perks even though Ghostlink does not
      project plugs. Armor and weapon mods were not compared.
  - FINDING F10 (high, stale state): after the PvP apply, forced syncs returned
    old (rev 21, Revenant), new (rev 22, Nightstalker), old again (rev 23, stable
    for about a minute) and only then new (rev 24, stable over four syncs), with
    no dispatch in between and DIM showing PvP. Bungie can serve stale profile
    replicas for minutes after an action; a sync that returns them overwrites a
    correct acknowledged projection and marks it `fresh` for the 900 s TTL.
    Bungie also lagged 20-30 s behind a DIM move. Same family as F9 and #71.

  F10 A + F9 B fix (2026-10-13, branch `fix/sync-never-backwards`, commit
  `4b505a8`, native review approved). Decision: apply the minted-timestamp guard
  to the explicit `ghostlink sync` too (A), and report a read whose sync was
  discarded as `stale`, never `fresh` (B); the action watermark (C) waits for
  evidence. A discarded explicit sync exits 0 with `publication:
  "discarded_not_newer"`; the output adds `profile_minted_at`. Preflight and
  post-action syncs still publish (their observation contract needs redesign).
  - Live probe (release build of the branch; log in
    `~/.cache/ghostlink-f3f4-backup/runs/c-probe-*.log`): baseline sync minted
    `...538.5`, apply Stasis at `...539.6`, then 12 forced syncs every 15 s.
    Sync 1 (+17 s) published the first post-action profile, minted `...555.6`
    (16 s after the action): Revenant. Syncs 2-11 (+33 to +177 s) received ten
    consecutive responses minted no later than `...555.6` and discarded them all;
    the projection stayed Revenant. Sync 12 (+193 s) published a profile minted
    `...636.6`, delivered 95 s after it was minted. A PASS. The pre-action window
    that C would close (a replica minted after the last sync but before the
    action) was 1 s here and was not observed. DIM did not show the change during
    the probe (its own refresh cadence); the Revenant state was read from the
    profiles Bungie returned, not from Ghostlink's projection.
  - Observed: Bungie's minted instant ran 1-2 s ahead of the local acquisition
    clock (`checked_at 1791918263`, minted `1791918265.350`), so C cannot compare
    the local action time with Bungie's minted time without a margin or a common
    clock source.
  - The Stasis apply in the probe exited 42 with the old persistence message:
    this branch is based on `main` without the F4 fix (PR #86).

## Follow-ups (out of scope)
- Windows `ghostlink://` handler (next feature, developed on Windows).
- winget / AUR / MSI / code signing.
