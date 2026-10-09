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
- [ ] **P1 — Indexed definition search (B7).** Resolve name/perk/type text
  through an indexed normalized column (or FTS5) and batched tag reads instead
  of reconstructing every definition.
- [ ] **P2 — Owned search in SQL (B8).** Push filters, ordering and limit to
  SQL (or bounded top-k), memoize base definitions per search.
- [ ] **P3 — Batched component reads (B9).** Read plugs/stats/sockets for
  many items in one query; `prepare_cached` on hot statements; narrow read
  surfaces for queries that rebuild full snapshots.
- [ ] **P4 — Diff sync writes (B10).** Write only changed rows on sync;
  skip the saved-loadout rewrite when unchanged.
- [ ] **R1 — Bungie read errors and throttling (A1).** Classify body
  `ErrorCode` on reads (throttle 1672/36/51, maintenance, auth) into typed
  errors with clear messages; DIM adaptive backoff (1 s → 5 min, success
  halves) shared per process; never auto-retry writes.
- [ ] **R2 — MCP read concurrency (A2).** Reads run concurrently against the
  projection; only mutations (and syncs) serialize, as DIM's action queue.
- [ ] **U1 — Compact item summary (A4).** DIM-style list summary (id, name,
  power, rarity/tier, plugged perks) across CLI/MCP list surfaces; detail on
  inspect.
- [ ] **U2 — Search continuation and inspect by name.** Cursor/offset for
  truncated results; `inspect-item` accepts a name and returns candidates when
  ambiguous.
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
