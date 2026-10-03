# Ghostlink Search Query (WU-027)

## Objective and scope
Make `search-inventory` and `inspect-item` (CLI and MCP, scriptable via `--json`) good enough to query the owned inventory precisely:
- repeated `--perk` / `--mod` combine with AND (god-roll queries);
- the whole inventory fits in one search (limit cap above the 665 owned items);
- `inspect-item` shows named plugs, archetype, available alternatives and stats, like search hits;
- `--tier` filter (e.g. Exotic);
- sort hits by a stat; filter by location (vault/character) and equipped state;
- invalid arguments explain which argument is wrong and why; strict `>` / `<` stat operators accepted.

Non-goals: OR/negation filters, pagination cursors, weapon frame filter, armor available mods (separate follow-up).

## Decisions
- Repeated `--perk` / `--mod` terms resolve independently (exact, then substring, then suggestion) and all must match (AND); inside one term, any resolved name matches (unchanged). Bounded like `MAX_STAT_CONDITIONS`. `--archetype` stays single (one archetype per armor piece).
- MCP stays backward compatible: existing single-string fields keep working; repeatable terms accepted without breaking current callers. CLI/MCP parity tests updated.
- Limit cap raised from 100 to 1000 (vault plus characters fit); no pagination.
- Sorting by stat happens in Rust after matching and before truncation; descending by default, ascending opt-in; items without the stat sort last; ties by instance id.
- `--tier` is exact normalized match on the stored definition tier; no migration (column exists).
- Location filter: vault or character; equipped filter: true/false. Applied in shared matching (fake and SQLite parity).
- Invalid-argument errors carry a bounded, sanitized detail naming the argument and the reason, in CLI text, CLI JSON and MCP; exit code unchanged (2).
- `inspect-item` reuses the per-instance enrichment used by search hits (one shared helper).

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Fake and SQLite repositories keep identical matching; CLI and MCP stay in step.
- Checks: fmt, workspace all-features tests, clippy with and without `--all-features`, diff check.
- Live validation against the real DB with the release binary.
- Delivery: work-unit commits on `feature/ghostlink-search-query`; push/PR/merge are user decisions.

## Tasks
- [x] **GSQ-001 — Establish tracker and branch.** Branch `feature/ghostlink-search-query` from `main` at `123d832`.
- [x] **GSQ-002 — Specific invalid-argument errors and strict stat operators.** Detail-bearing invalid-argument error (CLI text/JSON, MCP); clap parse failures and `into_request` failures name the argument and reason; `>` and `<` supported in stat conditions. Delegated writer, test-first (RED: CLI generic message, MCP generic limit message, `Range>60` rejected). Bounded control-free detail constructor in core; clap errors mapped by kind and argument id without echoing values (auth secrecy test). Checks (writer): fmt, workspace all-features 1,109 passed, clippy both variants, diff check. Parent live check (release): `--limit 0` -> `invalid --limit: must be between 1 and 100`; `--slot banana`, `--stat Range>=abc`, repeated `--perk` explained; JSON message carries detail; `--type "Hand Cannon" --stat "Range>60"` -> 7 hits. Commit: b1f181a.
- [x] **GSQ-003 — Repeatable perk and mod terms (AND).** CLI `--perk`/`--mod` repeatable, MCP accepts several, core criteria become bounded lists, resolution per term, `resolved` output per term, include-available handling. Delegated writer, test-first (RED: clap ArgumentConflict on repeated `--perk` in parity test; core list API type errors). Cap 8 terms per kind; archetype stays single and socketed-only. MCP keeps `perk_name`/`mod_name` and adds `perk_names`/`mod_names` (combined with AND, shared cap); `resolved` keeps flat `perk`/`mod` and adds `perk_terms`/`mod_terms`. Checks (writer): fmt, workspace all-features 1,111 passed, clippy both variants, diff check. Parent live check: `--perk "Kill Clip" --perk "Ricochet Rounds"` -> Antiope-D, Tigerspite, Pillager; plus `--type "Auto Rifle"` -> Tigerspite; Demolitionist + Adrenaline Junkie -> 0 (disjoint sets, 7 vs 3 items); typo in second term suggests. Commit: 0e40cbe.
- [ ] **GSQ-004 — Tier, location and equipped filters.** `--tier`, `--location vault|character`, `--equipped true|false` (CLI and MCP), shared matching, parity tests.
- [ ] **GSQ-005 — Sort by stat and higher limit.** `--sort <Stat>[:asc]`, cap 1000, collect-sort-truncate in both repositories.
- [ ] **GSQ-006 — Full inspect-item.** Shared enrichment helper; inspect shows named perks/mods, archetype, available alternatives and stats (CLI text/JSON, MCP).
- [ ] **GSQ-007 — Docs, verification and live validation.** README, full checks, live query battery, close tracker.

## Progress
QA 2026-10-03 (release build, real DB): composed filters work; gaps found are the six items above. Exploration map: see Engram `odd/ghostlink-search-query/tasks`.

## Next step
GSQ-004.
