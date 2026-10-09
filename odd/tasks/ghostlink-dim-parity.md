# DIM Parity — Read Performance and UX — IN PROGRESS

Second DIM comparison round (2026-10-09), after the rc.3 action-model
migration. Goal: faster reads on SQLite and DIM-grade CLI/MCP experience.

## Scope and decisions (owner, 2026-10-09)

- **Reference model: Destiny Item Manager (DIM).** Clone at
  `/tmp/dim-research-2/DIM` (read-only; re-clone with `git clone --depth 1`
  if missing). For any design question, check DIM first; apply directly
  when it fits, discuss with the owner only when ambiguous or it does not
  fit Ghostlink (CLI process per command, SQLite projection).
- Exploit SQLite: indexed lookups, SQL-side filtering/ordering, batched
  reads, cached statements, diff writes.
- All user-facing text in English.
- Test-first where applicable; gates `cargo test --workspace --all-features`,
  fmt, clippy `-D warnings`, `git diff --check`.
- One work-unit commit per task on a feature branch; push/PR/merge are
  owner decisions.

## Findings map (evidence: Engram `architecture/ghostlink-dim-improvements-round2`)

B7 N+1 definition search · B8 sort-all-then-truncate · B9 per-item component
queries · B10 full projection rewrite · A1 read throttling as provider
validation · A2 global MCP mutex · A3 DIM search language · A4 compact item
summary · A5 loadout phases · A6 serial Manifest downloads.

## Tasks

- [x] **P0 — Baseline timings.** Reproducible timing harness over a large
  synthetic catalog + projection (search by name, no-match search, stat-sorted
  search, inspect, list, unchanged/changed sync). Harness:
  `crates/destiny-adapters/tests/perf_baseline.rs` (`#[ignore]`; run with
  `cargo test -p destiny-adapters --release --test perf_baseline -- --ignored
  --nocapture --test-threads=1`). Dataset: 20,000 definitions, 600 instances,
  300 stacks, 3 characters, 30 saved loadouts; median of 9 samples.
  Baseline (2026-10-09, release, `main` e8d8c50):

  | measurement | median ms |
  |---|---:|
  | definition name (late match) | 131.5 |
  | definition name (no match) | 132.1 |
  | owned text | 2.3 |
  | owned stat sort (limit 5) | 10.7 |
  | owned perk name | 16.1 |
  | inspect item (service) | 8.7 |
  | full committed snapshot | 8.4 |
  | publish unchanged | 9.4 |
  | publish one changed item | 80.1 |
- [x] **P1 — Indexed definition search (B7).** Resolve name/perk/type text
  through an indexed normalized column (or FTS5) and batched tag reads instead
  of reconstructing every definition. Done: migration 22 adds the STRICT
  `definition_search` companion (Rust-normalized name/type/tier/tags,
  attributes, name-kind flags), maintained inside catalog publication and
  backfilled atomically; search prefilters in one cached SQL statement
  (`instr` substring, exact attributes, delimited tag membership) and
  reconstructs only candidates, with `definition_matches` as final arbiter;
  `name_catalog` reads names from the companion. Coverage mismatch fails
  closed (`LocalStorage`). DIM equivalent: linear predicate over compact
  normalized data. Definition name search 131.5 ms -> 0.94 ms; no-match
  132.1 ms -> 0.90 ms. Workspace: 1966 passed, 0 failed.
- [x] **P2 — Owned search in SQL (B8).** Push filters, ordering and limit to
  SQL (or bounded top-k), memoize base definitions per search.
- [x] **P3 — Batched component reads (B9).** Read plugs/stats/sockets for
  many items in one query; `prepare_cached` on hot statements; narrow read
  surfaces for queries that rebuild full snapshots.
  P2+P3 done together (same root cause: per-item reconstruction). DIM
  equivalent: predicate over items already loaded with their components.
  Set-based component reconstruction with shared validation (full reads also
  fail closed on orphan component rows, which FK cascades make impossible
  outside corruption); instance search reads 64-candidate batches (500 when
  sorting), memoizes base definitions, keeps a stable bounded top-k with the
  existing comparator; `prepare_cached` on hot statements; 620-item parity
  matrix in `tests/stat_search.rs`. Release medians vs P0: owned text
  2.3 -> 1.6, stat sort 10.7 -> 5.1, perk name 16.1 -> 10.9, inspect
  8.7 -> 5.0, snapshot 8.4 -> 4.7, publish unchanged 9.4 -> 5.7 ms.
  Workspace: 1968 passed, 0 failed.
- [x] **P4 — Diff sync writes (B10).** Write only changed rows on sync;
  skip the saved-loadout rewrite when unchanged. Done: publication diffs
  against the reconstructed prior snapshot inside the same IMMEDIATE
  transaction (characters by id, items by instance id with their components
  replaced as a unit, stacks by schema ordinal, subclass options by
  scope/hash); equal saved loadouts are not rewritten. Revision, freshness,
  minted timestamp, invalidation and acknowledged effects unchanged. Tests:
  20-step sequence parity against fresh-database publication, trigger guards
  on untouched rows, rollback trigger moved from INSERT to DELETE because a
  removed option is now a single DELETE. DIM replaces its store wholesale in
  memory; on SQLite the equivalent is a row diff. Publish one changed item
  75.0 -> 5.9 ms. Workspace: 1971 passed, 0 failed.
- [x] **R1 — Bungie read errors and throttling (A1).** Classify body
  `ErrorCode` on reads (throttle, maintenance, auth) into typed
  errors with clear messages; DIM adaptive backoff (1 s → 5 min, success
  halves) shared per process; never auto-retry writes.
  Owner decision (2026-10-09): DIM's 5-minute wait does not fit a CLI/MCP
  tool call; cap the pre-request wait at 30 s and, when the computed wait
  exceeds it, fail fast with ProviderRateLimit and an English message with
  the retry time (from `ThrottleSeconds` when present). Throttle code set =
  DIM's (35, 36, 37, 54, 55, 56, 57, 1672); 5 maintenance; 99/22/2111/2123/
  2124 reauthorization; 2101/2102 configuration.
  Done: `bungie_throttle.rs` shared process-wide counter (mutex, injectable
  clock/sleeper) used by the profile, membership, Manifest and action
  transports; OAuth token requests excluded as in DIM (`oauth.ts` plain
  fetch). Waits above 30 s refuse without dispatch; after the computed
  cooldown (or a longer `ThrottleSeconds`) a caller-driven probe is allowed,
  since DIM always sends after waiting and the counter only heals on
  success. Pre-dispatch refusal of an action is definitive (zero attempts,
  no invalidation) and keeps the "retry in about N seconds" diagnostic
  through `MutationError`. Reads keep the existing bounded retry caps;
  writes are never retried; auth codes are classified only (no new
  refresh/replay). Workspace: 1983 passed, 0 failed.
- [x] **R2 — MCP read concurrency (A2).** Reads run concurrently against the
  projection; only mutations (and syncs) serialize, as DIM's action queue.
  Done: four round-robin read slots with their own SQLite connections (WAL
  snapshots) sharing one profile gateway/session; fresh reads no longer wait
  for mutations. Read-triggered syncs take the exclusive guard shared with
  mutations through an optional `ProfileSyncService` sync gate and re-check
  freshness after acquiring it (single-flight, as DIM's `loading` guard in
  `d2-stores.ts:122-188`); mutation syncs do not take the gate (they already
  hold it). Lock order: read slot -> exclusive -> adapter; exclusive ->
  mutation service -> adapter. rc002 publication-order tests adapted; mcp
  suite 20/20 clean runs. Workspace: 1986 passed, 0 failed.
  Also: R1 follow-up `65def0c` caps the throttle cooldown at five minutes
  (DIM cap; review advisory found ~6 days after 20 throttles).
- [x] **U1 — Compact item summary (A4).** DIM-style list summary (id, name,
  power, rarity/tier, plugged perks) across CLI/MCP list surfaces; detail on
  inspect. Done: inventory search hits default to a shared compact tile
  (`destiny-output` projection: instance_id, item_hash, name, item_type,
  tier, element, power, location, equipped, locked, quantity when > 1,
  plugged perk/mod names, and only the stats referenced by the query's
  filters/sort); CLI `--detail` and MCP `detail: true` keep the previous full
  shape. CLI default is one line per hit. Candidates, inspect, loadout
  detail, subclass/artifact and Postmaster unchanged (Postmaster has no
  power/perks to summarize). Enriched fixture hit 617 -> 310 B. Existing
  full-output pins now opt into detail. Workspace: 1992 passed, 0 failed.
  Open (not U1): MCP text + structuredContent duplication is by spec (G1);
  revisit with client support evidence.
- [x] **U2 — Search continuation and inspect by name.** Cursor/offset for
  truncated results; `inspect-item` accepts a name and returns candidates when
  ambiguous. Done: owned hits accept `offset` (CLI `--offset`, MCP `offset`,
  0..1000) in the existing order (stat-sorted top-k bound offset+limit+1,
  lookahead limit+1) and return `next_offset` only when more hits exist; CLI
  prints `More results: rerun with --offset N`. `inspect-item --name` / MCP
  `name` inspect a unique exact normalized owned name; several matches return
  `ambiguous_match` with compact U1 tiles (DIM shows candidates); fuzzy
  matches are never inspected. Exactly-one-selector is enforced at runtime:
  the generated top-level `oneOf` was removed because the Anthropic API
  rejects tool input schemas with top-level oneOf/anyOf/allOf (400 on every
  request; anthropics/claude-code #4886, #45106). Workspace: 1999 passed,
  0 failed; owned search timings unchanged.
  Follow-up (review advisories): offset has its own ceiling
  `MAX_SEARCH_OFFSET = 10_000` (accounts can exceed 1000 owned instances)
  across core, CLI and MCP; `next_offset` is only emitted when the next
  request would be accepted, and the default page fallback never probes past
  the ceiling. Workspace: 2006 passed, 0 failed.
- [x] **Fix — `apply_loadout` top-level `oneOf` (found during U2).** The
  shipped rc.3 `apply_loadout` input schema had a top-level `oneOf`, which
  the Anthropic API rejects with a 400 on every request (unusable sessions in
  Claude clients). Removed; slot/name exclusivity stays enforced at runtime
  (`ApplyLoadoutArgs::request`, covered by `tests/stdio.rs`). New guard test:
  no advertised tool input schema may use top-level oneOf/anyOf/allOf (RED on
  the previous code). Workspace: 2001 passed, 0 failed.
- [ ] **U3 — DIM search language (A3).** Query AST (`is:`, `perk:`,
  `stat:`, negation, parentheses, `or`, DIM precedence) driven by one filter
  registry that also provides validation and help; invalid queries fail
  explicitly. Plan sub-slices before implementing.
- [ ] **U4 — Loadout apply phases (A5).** Compare `apply_loadout` with DIM
  (dequip → moves → bulk equip → subclass/mods, per-item partial failure);
  implement the gaps.
- [ ] **U5 — Small polish (A6).** Manifest small tables with concurrency 4;
  shorter MCP tool descriptions; stale "four reads" comment.
- [ ] **D — Docs, skill and live checklist.**

## Delivery

- Branch `perf/sqlite-reads` from `main` at `e8d8c50` for P0–P4; later groups
  stack on it.

## Evidence
