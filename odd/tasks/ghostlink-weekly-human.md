# Ghostlink Weekly for Humans (WU-021)

## Objective and scope
Make `ghostlink weekly` read like a player's dashboard instead of a serialization of Bungie's structure, and add `--details` and `--debug` levels. `--json` stays the complete structured contract.

Principle: Ghost Link improves readability but never claims a taxonomy the API does not expose. Layers: Bungie API/Manifest -> raw model -> normalization -> derived presentation hints -> renderer. Derived hints are labeled as derived.

Non-goals: translating objective texts; claiming raid encounter challenges (they arrive mixed into `modifierHashes`); expanding `{var:...}` templates; interpreting reward names (for example "Deepsight Weapon (Chance)"); new weekly categories.

## Output levels
- default: what is featured this week and what to do. No modifiers, no manifest version, no hashes.
- `--details`: default plus modifiers grouped by derived category.
- `--debug`: details plus hashes, `resolved` flags, every raw entry (no name deduplication), manifest version, and unclassified activities.
- `--json`: unchanged complete contract plus the new structured fields below.

## Rules
- Header: `Ghost Link — Weekly` then `<Class> · Power <light>`.
- Sections in order: GRANDMASTER, FEATURED RAIDS, FEATURED DUNGEONS, VANGUARD, CRUCIBLE, SEASONAL. No activity-type suffixes inside sections.
- One entity per activity root; non-default variants (for example Master) render as sub-blocks only where they differ.
- Objectives: Bungie `progressDescription` verbatim with `○` (incomplete) or `✓` (complete) and `progress/completion`. No translation.
- Raid phases render as `Encounters n/m`.
- No personal objective in the payload (for example Deep Stone Crypt for the Hunter): render nothing for it, never fill in.
- Rewards and modifiers: deduplicate by exact name in default and `--details`; `--debug` keeps every entry with its hash.
- Quickplay: one line per distinct reward pool, `○ <objective> → <rewards>`, never merged into one activity.
- Modifier categories (derived, name-based, English): Surge (name ends with "Surge"), Overcharge (name starts with "Overcharged"), Champions (name contains "Champion"), everything else "Other modifiers". JSON exposes it as a derived `category` field.
- Unclassified activities: list of `{hash, name|null, reason, source}`; reason `activity_definition_missing` or `activity_type_missing`; source `public_milestones` or `character_activities`. Default and `--details` show one warning at the end (count line plus the `ghostlink sync-manifest` hint line, as agreed with the user); `--debug` lists them.

## Constraints and acceptance
- Classification rules from WU-020 are unchanged; this WU changes the model only where listed and the renderer.
- Rendered outputs pinned by tests for each level; CLI/MCP JSON parity kept.
- Test-first for behavior tasks. Checks per task: `cargo fmt --all -- --check`, `cargo build -p destiny-cli -p destiny-mcp`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Worktree `/home/yoryo/apps/ghostlink-human` with its own `target/`; the user's `~/apps/ghostlink` binary is untouched until merge.

## Tasks
- [x] **WHU-001 — Model.** Delegated writer, test-first (RED: missing unclassified types, `modifier_category`, `pools`; GREEN). Unclassified list `{hash, name|null, reason, source}` (definition missing takes precedence; `unclassified_unresolved` = list length kept); Quickplay `pools` merge only identical challenges and identical exact-name reward sets ([A] and [A, A] merge, consistent with the dedupe rule); derived `category` per modifier, commented as name-based. CLI/MCP parity and MCP schema updated; text layout unchanged. Independent verifier FAILED on coverage only (the `contains("Champion")` rule was not discriminated) -> parent added "Match Game: Champions" and proved it with a mutation (`starts_with` failed the test). Checks: fmt, workspace tests, clippy, diff check. Commit evidence: see git log for `feat(weekly): model unclassified activities, quickplay pools and modifier categories`.
- [x] **WHU-002 — Default renderer.** Delegated writer, test-first (RED: six human-render assertions; GREEN). New `crates/destiny-cli/src/weekly_human.rs` renders the dashboard from app-classified groups and pools; JSON unchanged. Parent live run on a database copy: layout as specified; Quickplay shows three pools because the live Normal and Master variants of the second pair differ by "Ops Bonus Drop" (faithful to the data, kept). Parent fixes: one-line entities listed adjacently (RED then GREEN). Independent verifier FAILED: (medium) objective dedupe ignored `complete` -> added to the key (RED: test failed, GREEN); (low) mixed sections with one-line entities still get gaps -> accepted, no such section exists today; (low) warning is two lines -> tracker updated to the agreed two-line form. Checks: fmt, workspace tests, clippy, diff check. Commit evidence: see git log for `feat(cli): render weekly as a player dashboard`.
- [x] **WHU-003 — `--details` and `--debug`.** Delegated writer, test-first (RED: missing level APIs, then three assertion failures; GREEN). Weekly-only, mutually exclusive flags, rejected with `--json` (`invalid_request`) before any provider composition; default output and JSON unchanged. `--details`: note "Modifier groups are derived from names.", modifiers grouped Surge/Overcharge/Champions/Other modifiers with common block plus per-variant additions (for example Master). `--debug`: manifest line, hashes everywhere, no name dedupe, UNCLASSIFIED section (live: activity 2158644986, `activity_definition_missing`, `character_activities`). Verifier FAILED: renderer derived categories -> category stored on the modifier in destiny-app. Parent live run fixes: unnamed modifier #1783825372 hidden in `--details` with a count line (`--debug` keeps it); shared Quickplay pools render one Modifiers block, differing pools are labeled by rewards; debug objectives one line with trailing hash. Re-verify FAILED on coverage (one-block rule) -> parent added a block-count assertion and proved it with a mutation. Checks: fmt, workspace tests, clippy, diff check. Commit evidence: see git log for `feat(cli): add weekly --details and --debug levels`.
- [ ] **WHU-004 — Live validation, docs, close.** Live run for the Hunter on a database copy; README.

## Next step
WHU-004 (waiting for the user's go-ahead).
