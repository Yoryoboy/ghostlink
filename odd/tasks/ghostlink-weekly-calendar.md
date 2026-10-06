# Ghostlink Weekly Rotation Calendar

## Objective and scope
Predict the featured weekly rotations that the Bungie API does not expose
(Pantheon) or exposes only for the current week (raids, dungeons, Grandmaster
Vanguard Alert), and let `weekly-reset` answer for any date with
`--date YYYY-MM-DD` (CLI) and the MCP equivalent.

Non-goals: weapon rotations (SV `gmWeapon` disagrees with Kyber on 2026-09-22
and 2026-09-29), Crucible/Ascendant/Europa/Expedition/Override calendars,
remote calendar updates, RFC 3339 datetime input, the WeekModel /
CharacterOverlay redesign.

## Decisions (user, 2026-10-06)
- Algorithm: ShatteredVault weekly engine
  (https://shatteredvault.com/the-portal/, fetched 2026-10-06).
  `week = floor((t - 2026-09-22T17:00Z) / 7d)`,
  `entry = list[(anchor + week) mod len]`, one independent list and anchor
  per rotation. Kyber's Corner has no engine; its weekly data is hand-edited
  static HTML (15 external scripts are WordPress/Jetpack/ads).
- Data lives in an embedded TOML file compiled into the binary
  (`include_str!`), not Rust constants and not a remote fetch. Entries carry
  Manifest activity hashes; names come from the Manifest.
- Dungeon slot 5 diverges from SV: Sundered Doctrine + Spire of the Watcher
  (SV: + Grasp of Avarice). Evidence: SV's list has Spire once and Grasp three
  times while every other dungeon appears twice; Kyber showed Sundered+Spire
  on 2026-07-14 and 2026-09-22 (exactly 10 weeks apart). Re-check on
  2026-12-01; revert in one line if Grasp appears.
- `--date YYYY-MM-DD` (ISO 8601). A date means the weekly period in effect at
  the end of that UTC day, so a Tuesday yields the week starting that
  Tuesday's reset. Output always shows the resolved window.
- Dates before a rotation's coverage start return "no data" with the reason
  (for example, the Pantheon weekly began 2026-06-16).
- `weekly-reset` requires login (user decision).
- Every calendar value is labeled as a prediction; live API data for the
  current week stays labeled as observed.

## Exploration findings
- CLI: Clap derive; `Command::WeeklyReset` has no args
  (`crates/destiny-cli/src/command.rs:215`); factory
  `crates/destiny-cli/src/runtime.rs:511-529` is credential-free today.
- App: `crates/destiny-app/src/weekly_reset.rs` has no Clock; it shows the
  provider's milestone windows. Core `Timestamp(u64)`
  (`crates/destiny-core/src/state.rs:8`), Clock port `ports.rs:20`.
- Core and app have no time or serde dependencies; chrono is adapter-only;
  no TOML dependency yet. Embedded-asset precedent:
  `crates/destiny-adapters/src/agents/mod.rs:12-13`.
- `WorldDefinitionRepository::lookup_activity(hash)` exists
  (`crates/destiny-core/src/ports.rs:346`); there is no lookup by name.
- MCP `weekly_reset` takes empty args with `deny_unknown_fields`
  (`crates/destiny-mcp/src/schema.rs:204-207`); production MCP already loads
  credentials at startup (`crates/destiny-mcp/src/main.rs:84-111`).
- Local Manifest has dedicated Pantheon hashes, e.g.
  `Featured Reprise: Gahlran: The Pantheon` 1953549041; raids, dungeons and
  GM strikes have many variants per activity.

## Constraints and checks
Test-first for behavior (RED, then GREEN). Checks per task:
`cargo fmt --all -- --check`, `cargo build -p destiny-cli -p destiny-mcp`,
`cargo test --workspace --all-features`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`git diff --check`. Worktree `/home/yoryo/apps/ghostlink-calendar`, branch
`feature/ghostlink-weekly-calendar` from `origin/main` `c353045`. Work-unit
commits; push/PR/merge are user decisions. Expected size exceeds 400 lines:
deliver as chained PRs.

## Tasks
- [ ] **WRC-001 — Calendar data.** Choose one canonical Manifest activity
  hash per raid, dungeon, Pantheon boss and GM strike (evidence from the local
  Manifest and previously observed live hashes), write the embedded TOML with
  lists, anchors, reference reset, per-rotation coverage start, the slot-5
  correction and provenance comments.
- [ ] **WRC-002 — Core calendar arithmetic.** Pure types and week-index
  computation (signed floor division, Euclidean modulo, dates before the
  reference reset), coverage-start "no data", window for a week. Pinned
  vectors: 2026-09-22, 09-29, 10-06, 10-13 and a pre-coverage date.
- [ ] **WRC-003 — Embedded TOML parsing and validation (adapters).** Parse
  and validate (nonempty lists, anchor in range, pair shape, every hash
  resolves in the Manifest in a data test, each raid/dungeon appears exactly
  twice).
- [ ] **WRC-004 — App and CLI.** `weekly-reset --date`, calendar section in
  human and JSON output, resolved window, live milestones only for the
  current week, require login.
- [ ] **WRC-005 — MCP.** Optional `date` argument, schema, stdio tests,
  offline fixture.
- [ ] **WRC-006 — Docs and skill.** CLI/MCP docs, README credit to
  ShatteredVault, skill guidance (predictions vs observations).
- [ ] **WRC-007 — Live validation.** Run against today's reset
  (2026-10-06 17:00 UTC) and compare with in-game/Kyber data.

## Follow-ups
- Warn when the current-week prediction disagrees with live observation
  (raid challenges, dungeon Trending status).
- Re-check dungeon slot 5 on 2026-12-01.
