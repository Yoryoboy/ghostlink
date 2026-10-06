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
  the end of that day in the system local time zone (CLI); MCP uses UTC.
  In Argentina a Tuesday yields the week starting that Tuesday's reset; in
  UTC+10 and later, local Tuesday ends before the reset, so Wednesday yields
  the new week. Output always shows the resolved window, in local time for
  humans and RFC 3339 UTC in JSON.
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
- [x] **WRC-001 — Calendar data.** Choose one canonical Manifest activity
  hash per raid, dungeon, Pantheon boss and GM strike (evidence from the local
  Manifest and previously observed live hashes), write the embedded TOML with
  lists, anchors, reference reset, per-rotation coverage start, the slot-5
  correction and provenance comments.
  Result: `crates/destiny-adapters/src/calendar/rotations.toml`. Each
  activity has a short display name plus one hash, because Manifest names are
  unsuitable for display ("Vow of the Disciple: Standard", "Featured Reprise:
  Gahlran: The Pantheon"). Hash sources: raids from live GetPublicMilestones
  Standard hashes (2026-10-06); dungeons from the hashes probed via
  `/Trending/Details` in the weekly research; Pantheon from the dedicated
  Featured Reprise/Encore activities; GM from "Nightfall Grandmaster: X"
  (Exodus Crash and Warden of Nothing have two definitions each; picked the
  ones sharing The Arms Dealer's current modifier layout). Coverage start:
  raids/dungeons/Pantheon 2026-06-16 (Bungie TWID June 18 pairs and Update
  9.7.0); GM 2026-09-22 (only Kyber evidence). Structural check (passive
  data, no meaningful RED; python tomllib + local Manifest): 32/32 hashes
  resolve with matching names, no unused activities, raids and dungeons each
  appear exactly twice; reconstructed 2026-06-16, 07-14, 09-22, 09-29 match
  TWID/Kyber. Commit evidence: `a7a726b` (data; this tracker note landed
  with WRC-002).
- [x] **WRC-002 — Core calendar arithmetic.** Pure types and week-index
  computation (signed floor division, Euclidean modulo, dates before the
  reference reset), coverage-start "no data", window for a week. Pinned
  vectors: 2026-09-22, 09-29, 10-06, 10-13 and a pre-coverage date.
  Result: `crates/destiny-core/src/calendar.rs` (dependency-free):
  validated `ActivityReference`, `Rotation`, `Calendar`; `week_index`
  (i128 `div_euclid`), `Rotation::slot_index` (`rem_euclid`),
  `week_window` (typed `WindowOutOfRange` instead of panics),
  `Calendar::week_at` -> `RotationOutcome::{Predicted, NoData}` with NoData
  when the week start precedes `coverage_start`. Delegated writer,
  test-first: RED 63 compile errors (`RotationOutcome` missing); GREEN
  destiny-core 136 tests (6 new: pinned raid/Pantheon vectors incl. week -14,
  reset boundaries, coverage, invalid constructors, extremes); fmt, clippy
  `-D warnings`, diff check passed. Commit evidence: see git log for
  `feat(core): add weekly rotation calendar arithmetic`.
- [x] **WRC-003 — Embedded TOML parsing and validation (adapters).** Parse
  and validate (nonempty lists, anchor in range, pair shape, every hash
  resolves in the Manifest in a data test, each raid/dungeon appears exactly
  twice).
  Result: `crates/destiny-adapters/src/calendar/mod.rs`,
  `embedded_calendar() -> Result<Calendar, CalendarLoadError>`; strict serde
  (`deny_unknown_fields`), typed errors for undefined/unused activities and
  bad instants, slot shapes, canonical order raid -> dungeon -> pantheon ->
  grandmaster (TOML maps are unordered). New dependency `toml =0.9.8`,
  default features off, `parse` + `serde` (MIT OR Apache-2.0; `winnow` MIT).
  Manifest hash resolution stays the WRC-001 structural check (a Manifest is
  not available to unit tests). Delegated writer, test-first: RED 8 failing,
  1 passing against a stub; GREEN 9/9 (balance invariant, pinned 2026-10-06
  and 2026-09-22 predictions, pre-coverage NoData). Independent verifier
  PASS: fmt, `cargo build -p ghostlink -p destiny-mcp`, workspace tests
  1,461 passed / 0 failed, clippy `-D warnings`, diff check, licenses.
  Commit evidence: see git log for
  `feat(adapters): load embedded weekly rotation calendar`.
- [x] **WRC-004 — App and CLI.** `weekly-reset --date`, calendar section in
  human and JSON output, resolved window, live milestones only for the
  current week, require login.
  Result: app service takes the injected `Calendar`, the evaluation instant
  and now; the milestones gateway is called only when both fall in the same
  week. CLI resolves the date at local 23:59:59 (chrono `clock` feature in
  the CLI only; DST ambiguity/gaps resolved without panics), renders the
  window in local time, and composes `embedded_calendar()` plus the existing
  credential path (login required, same outcome as other authenticated
  commands). JSON is additive: `current_week`, `calendar {source, start, end,
  rotations[{key, title, status predicted|no_data, activities[{name, hash}],
  coverage_start?, coverage_note?}]}`, `milestones_source`; non-current weeks
  keep `milestones: []`. MCP composition root injects calendar and clock
  (service.rs stays adapter-free; scope extension approved by the parent).
  New activated crates: iana-time-zone and platform helpers, all MIT OR
  Apache-2.0. Delegated writer, test-first: RED exit 101 for missing APIs and
  timezone tests; GREEN pinned Argentina/Brisbane/UTC vectors, pre-coverage
  notes, zero gateway calls for other weeks, login parity. Checks: fmt,
  build `ghostlink` + `destiny-mcp`, workspace tests 1,470 passed / 0
  failed, clippy `-D warnings`, diff check (all exit 0). Parent spot check
  with the real binary and local DB: `--date 2026-10-13` -> VotD+LW,
  Vesper+Prophecy, Argos+Morgeth, Warden; `--date 2026-06-15` -> all four
  no data with notes; `06/10/2026` -> exit 2; `TZ=Australia/Brisbane`
  2026-10-06 -> previous week, 2026-10-07 -> week of 2026-10-07 03:00 +10:00.
  Commit evidence: see git log for
  `feat(cli): predict weekly rotations for any date`.
- [x] **WRC-005 — MCP.** Optional `date` argument, schema, stdio tests,
  offline fixture.
  Result: `weekly_reset` accepts optional `date` (`YYYY-MM-DD`, UTC end of
  day, documented in the schema); the description says calendar values are
  community-derived predictions and milestones are observed for the current
  week only; output schema gains `current_week`, `calendar`,
  `milestones_source` (additive). Invalid dates return typed
  `provider_validation`; unknown arguments stay -32602. UTC resolver
  composed in `main.rs`; `service.rs` stays adapter-free. Delegated writer,
  test-first: RED exit 101 (dated call rejected -32602, schema lacked
  `date`); GREEN stdio contract with zero milestones requests for
  non-current and pre-coverage dates, two for the two current-week calls,
  UTC unit test 2026-10-06 -> week starting 2026-10-06T17:00Z. Checks: fmt,
  build, workspace tests 1,472 passed / 0 failed, clippy `-D warnings`,
  diff check (all exit 0). Commit evidence: see git log for
  `feat(mcp): accept a date for weekly_reset predictions`.
- [x] **WRC-006 — Docs and skill.** CLI/MCP docs, README credit to
  ShatteredVault, skill guidance (predictions vs observations).
  Result: `docs/cli.md` (login, local end-of-day `--date`, UTC+10 Tuesday
  caveat, local window, prediction vs observed, coverage, additive JSON),
  `docs/mcp.md` (optional UTC `date`, same fields), README Credits
  (ShatteredVault algorithm and lists, Kyber's Corner historical data, the
  one corrected dungeon slot), skill `## Weekly Calendar` section and the
  weekly-reset table row now says login required. Delegated writer; passive
  docs, no meaningful RED. Checks: flags verified against `--help`, skill
  tests 4/4, workspace tests 1,472 passed / 0 failed, diff check (exit 0).
  Commit evidence: see git log for
  `docs(calendar): document weekly rotation predictions`.
- [x] **WRC-007 — Live validation.** Run against today's reset
  (2026-10-06 17:00 UTC) and compare with in-game/Kyber data.
  User confirmed all four rotations directly in-game. Existing branch binary
  `target/debug/ghostlink weekly-reset --json` executed at
  2026-10-06T17:07:24Z, exit 0, empty stderr, no rebuild. Window:
  2026-10-06T17:00:00Z -> 2026-10-13T17:00:00Z; `current_week: true`.
  All predictions match the user's observation: Salvation's Edge + Vault of
  Glass; Pit of Heresy + Warlord's Ruin; Calus + Consecrated Mind;
  The Arms Dealer. Live milestones returned the current reset window but
  do not independently identify the featured rotations; the game is the
  ground truth, not mere presence in the milestone list. Capture:
  `/tmp/ghostlink-weekly-reset.brAxif/stdout.json` (temporary evidence).
  This validates this reset only, not universal prediction accuracy or
  dungeon slot 5. Commit evidence: this delivery tracker commit.

## Delivery
- Approved issue: https://github.com/Yoryoboy/ghostlink/issues/56.
- User authorized commit, push, PR creation and merge after required CI.
- Strategy: stacked-to-main, selected by the user. Five cohesive slices:
  data/core, embedded parser, CLI, MCP, docs. Core/parser/CLI exceed the
  advisory 400-line budget; tests stay with their behavior. No repository
  size-exception policy exists; no size-exception label is applied.
- Each slice must pass Ubuntu, Windows and broker CI before merge.
- Reference #56 in slices 1-4; close it only with slice 5.

## Follow-ups
- Warn when the current-week prediction disagrees with live observation
  (raid challenges, dungeon Trending status).
- Re-check dungeon slot 5 on 2026-12-01.
