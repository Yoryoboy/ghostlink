# Ghostlink Weekly Personal (WU-020)

## Objective and scope
Add an authenticated `ghostlink weekly` command and a matching MCP read tool that report, for one character, the weekly content that the Bungie API supports reliably, plus the character's progress on it. Chained on WU-019 (`feature/ghostlink-weekly-reset`, `bcc6361`): reuses its world definitions, public milestones gateway, and SQLite v3 schema.

Only "reliable" signals from the live study (below) are shown. Ambiguous or contradicted data is omitted, never guessed.

Non-goals: loot tables beyond `visibleRewards`; Featured Reprise; Crucible and Arena Ops alert weapons; `isCompleted` as a weekly checklist; multi-character summaries (one character per call); changing the inventory profile request.

## Live study (2026-10-02, Hunter, components 200/202/204, throwaway probe deleted)
- 204 `availableActivities` (235 for the Hunter): `activityHash`, `isFocusedActivity`, `isCompleted`, `recommendedLight`, `difficultyTier`, `modifierHashes`, `challenges[].objective{objectiveHash, progress, completionValue, complete}`, `visibleRewards[].rewardItems[].itemQuantity.itemHash`. Reward hashes resolve against the items table to real names (for example Exodus Crash -> The Slammer).
- 202 `milestones` (13): like the public milestones plus `rewards[].entries[]{rewardEntryHash, earned, redeemed}` and per-activity `challenges` progress and `phases[]{phaseHash, complete}`.
- Response for three characters is about 950 KB.

## Reliability classification (compared with the in-game weekly screen)
Reliable:
- Featured raids: public milestones activities with challenges, excluding the newest raid.
- Featured dungeons: 204 dungeon activities with challenges, excluding the newest dungeon.
- Newest content: its visible reward is "Weekly Reward"; rotators show "Raid Gear" or "Powerful Gear".
- Grandmaster Vanguard Alert: the 204 mission at the highest recommended light with a challenge, with its reward weapon.
- Weekly Expedition, Reclaim, Override: the single available variant of each kind (one week of evidence; re-confirm on the next reset).
- Portal focused activities (`isFocusedActivity`) with their reward names.
- Challenge progress and raid phase completion; Nightfall visible drops.
Ambiguous (omit): two Quickplay Vanguard Op variants with different weapons; `isCompleted` (true while the weekly challenge is 0/1).
Contradicted (omit): Crucible alert weapon (API The Helmsman, game Randy's Throwing Knife); Featured Reprise (API Calus, game Gahlran); Deep Stone Crypt has no challenge in 204 for the Hunter although the public endpoint gives it one (raids come from the public endpoint).

## Constraints and acceptance
- Separate profile request for components 202 and 204 with its own exact allowlist; the inventory request string `components=102,200,201,205,300,302` stays unchanged.
- Uses the existing authenticated session and refresh path; no new config loader; eager MCP startup unchanged.
- Bounded typed decoding, sanitized errors, redacting `Debug`, no raw payloads stored or printed.
- Every classification rule has a named fixture test; each output section states its rule in the docs.
- Test-first for behavior tasks. Checks per task: `cargo fmt --all -- --check`, `cargo build -p destiny-cli -p destiny-mcp`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Live runs use a copy of the database (`DESTINY_DATABASE_PATH` override).
- Worktree `/home/yoryo/apps/ghostlink-weekly`, branch `feature/ghostlink-weekly-personal`; push/PR/merge are user decisions.

## Tasks
- [ ] **WPP-001 — Weekly profile domain and gateway.** Core snapshot types and port; adapter request for components 202/204 for one character with exact allowlist; bounded typed decoder; synthetic sanitized fixture mirroring the live shape.
- [ ] **WPP-002 — Objective and activity-type definitions.** Add `DestinyObjectiveDefinition` (progress description) and `DestinyActivityTypeDefinition` (name) to Manifest acquisition and storage.
- [ ] **WPP-003 — Weekly classification service.** App service combining the public milestones and the character snapshot into reliable sections (featured raids, featured dungeons, newest content, Grandmaster alert, Expedition/Reclaim/Override, Portal focus, challenges and phases), each rule tested.
- [ ] **WPP-004 — CLI `weekly`.** Character selection (default: most recently played), text and `--json`.
- [ ] **WPP-005 — MCP read tool.** `weekly` tool with schemas, dispatch, tool-list assertions.
- [ ] **WPP-006 — Live validation, docs, close.** Live run for the Hunter on a database copy; README; re-check rules against the next weekly reset if available.

## Next step
WPP-001.
