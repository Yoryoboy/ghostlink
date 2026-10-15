# Clarity Community Insights — COMPLETE

## Objective

Add Clarity's community-researched perk data (numeric values, corrections) as
a second, clearly attributed layer next to the official Bungie `description`,
so agents can calculate builds instead of guessing numbers.

Source: <https://github.com/Database-Clarity/Live-Clarity-Database>
(`live` branch). Terms: <https://www.d2clarity.com/partnerships>.

## Decisions

- **Always on.** No config flag. The repo is private today, but the public
  release machinery (PR #50) is merged: the user commits to closing a Clarity
  partnership (Discord ticket) before publishing any release that includes
  Clarity data.
- **Synced inside `sync-manifest`.** No new command. Clarity is checked after
  the Manifest; data is downloaded only when `versions.json` changes. A
  Clarity failure never fails or rolls back the Manifest publication; it is
  reported in the sync output.
- **Source file:** `descriptions/vaultZero.json` (plain text with `[Stasis]`,
  `[enh]` tokens like Bungie's notation). `crayon.json` contains Discord emoji
  markup and is not used.
- **Verbatim storage.** Text stored byte-for-byte; no trimming or whitespace
  normalization. Invalid/oversized entries are dropped whole, never edited.
  Presentation-only adjustments (indentation) happen at render time.
- **Own storage.** Separate table and version record (schema v15); the
  generic `sync_metadata` dataset enums are not widened.
- **Output:** optional `clarity` string next to `description` on the five
  plug surfaces (subclass, subclass options, artifact, loadouts, inspect
  `plug_details`). No per-response version/date.
- **Matching:** exact hash first; then same name within the same kind (e.g.
  Prismatic aspect copies); a different variant (enhanced vs base) is shown
  but marked as a variant match.
- **Credits:** README Credits section with Clarity, https://d2clarity.com and
  the Discord https://d2clarity.com/discord, plus docs and skill rule to cite
  Clarity when using its numbers.

## Evidence

- Raw URLs return 200 `text/plain; charset=utf-8`:
  `.../live/versions.json` (27 B), `.../live/descriptions/vaultZero.json`
  (~868 KB, 1,765 entries, 1,756 with `en` text, max 1,311 chars).
- Hash variants: Prismatic Ascension 2835214901 vs Clarity 4194622039 (Arc);
  enhanced Unburied Treasure 3804163130 vs Clarity base 3132659494.

## Tasks (chained PRs)

- [x] 1. PR A — data: fixed-URL transport, decode, v15 storage, sync inside
      `sync-manifest` with isolated failure; README Credits.
- [x] 2. PR B — exposure: `clarity` field on five surfaces with matching rule
      and variant marking; CLI JSON/human, MCP, parity tests.
- [x] 3. PR C — agents: docs and skill rule to cite Clarity.
- [x] 4. Deliver chained PRs after checks: A/B copied-DB live checks passed;
      C structural docs checks passed (runtime N/A). All PR CI passed and
      A/B/C merged. No public release or installed-binary update.

## Current state

PR A committed `8b18b98`; PR B committed `a5b30ab` after corrected live
acceptance; PR C committed `fe438a7` after documentation checks, with tracking
commit `83f2967`. Feature tracker #52 closed. All PRs merged into main:
A #53 merge `6ec3117`, B #54 merge `dc41105`, C #55 merge `c353045`.
B/C were retargeted to main only after their predecessor merged; each new
base differed from the tested parent solely in public-release task documentation.
All checks completed: broker/Linux/Windows/release-plan SUCCESS per PR;
four release publication jobs SKIPPED, not executed. Native RDD remains off.
All live checks used a copied DB; installed binaries and real DB unchanged.

Delivery chain: A data -> B five-surface exposure -> C agent guidance.
Changed-line budgets: A 959, B 1,240, C 76 (including task evidence).
Single slicing pass completed: A could split into three infrastructure units;
B could split into four units but the matching foundation alone remains about
550-650 lines. Preserve A/B/C is recommended, but the owner explicitly accepted the size exception for A/B via the
`keep_three_size_exception` selection. Delivered the existing three-slice chain. Alternative
chain would grow to eight PRs; intermediate rewritten bases are not verified.
No PR template exists; feature discussion/approval is recorded in issue #52. Native workflows do not validate
approved issue/type labels/branch regex; README requests issue discussion for
large changes. Gentle AI skill-specific policy is not native Ghostlink CI. Only pre-existing
inspection raw perk-hash parity difference remains a separate followup.

## Evidence log

- PR C: skill v1.2 adds Clarity source attribution, no invented numbers,
  variant-value safeguards and five optional surfaces. CLI/MCP docs match
  semantic fallback restrictions and remove stale exposure claim. Passive
  documentation has no meaningful RED/runtime check; frontmatter, 11 local
  references, safety/attribution assertions and diff whitespace checks passed.
  Automatic Markdown checks clean; anchors not separately validated.

- PR B post-correction acceptance passed: release build and focused tests
  exit 0 (MCP parity 9, inspection 5, core matching 12, app cache 4). All
  14 live CLI reruns exit 0; expected targets and verbatim text preserved.
  Loadouts 108 to 107, variants 3 to 2: only unsafe enhanced Radar Tuner
  weapon-mod name fallback was removed. Actual MCP six read calls succeed;
  four full envelopes equal and inspection plug_details/enriched name arrays
  equal. Full inspection envelope differs in raw `instance.perks.perk_hash`:
  confirmed pre-existing at base `8b18b98` (CLI output.rs:1204-1208 omits,
  MCP service.rs:1983-1986/schema.rs:846-850 emits). No unrelated repair.
  Positive artifact live match unavailable; omission verified. Original full
  suite passed before correction; focused core/app/MCP checks rerun afterward.
  Native RDD switch remains off; native assessment medium/large writer.

- PR B correction completed (uncommitted): semantic-kind filtering now
  precedes enhancement/parent/ambiguity preferences; exact hashes unchanged.
  Unsupported vocabulary omits name fallback. Two cross-category assertion
  regressions observed RED then GREEN; core library 130/app library 370
  tests passed, fmt and focused clippy passed. Initial app test command
  failed because `test-support` was missing; corrected feature invocation
  passed. Existing domain groups weapon frames/perks/traits together.
  Post-fix live verifier resumed; acceptance and commit remain pending.

- PR B pre-correction live verification (provisional): release build exit 0;
  14 live CLI commands exit 0 on copied DB. All 1,756 stored rows exactly
  equal upstream English text/name/kind/item metadata, no missing/extra rows.
  Emitted insight counts: Eyasluna 8 (enhanced Unburied Treasure variant),
  Orpheus Rig 1 (Uncanny Arrows intrinsic), subclass 10, Prismatic Hunter
  options 29 (Ascension/Threaded Specter unmarked name fallback), loadouts
  108. Every emitted text and human block matches verbatim source plus
  presentation indentation. Artifact has 0 insights: seven nonempty live
  perks have no catalog hash/name match, 17 slots empty; positive artifact
  live coverage unavailable. Real MCP stdio initialization/discovery and six
  read calls succeeded; CLI/MCP envelopes equal for four surfaces and
  plug_details equal for both inspections. Process exit 0, stderr empty.
  Focused parity tests: 9 passed; inspection tests: 5 passed. Logs/reports:
  `/tmp/ghostlink-clarity-acceptance.nICyXIyp/`. Post-correction rebuild and
  affected live/test reruns remain pending; verifier holds while writer edits.
- PR A — `clarity_insights` + singleton `clarity_metadata` (schema v15),
  fixed-URL transport (Bungie allowlist untouched), verbatim decode with exact
  raw version token, Manifest-first orchestration in explicit `sync-manifest`
  and CLI auto-sync; Clarity failure reported, never fatal. README Credits.
  Writer: compile-level RED (exit 101), GREEN; fmt 0, clippy 0, tests 0
  (1,425 passed, 0 failed).
  Live (copy of real v14 DB): migration to v15, `sync-manifest` exit 0,
  `clarity: updated`, version 2.0623, 1,756 stored / 9 skipped; second run
  `unchanged`. Stored rows vs fresh raw vaultZero.json: 1,756 expected,
  0 mismatches (text, name, kind), 0 extra. Auto-sync on an empty DB
  (`--json weekly-reset`) exit 0, Clarity populated, progress on stderr,
  stdout valid JSON. Not exercised live: network failure (covered by tests).
