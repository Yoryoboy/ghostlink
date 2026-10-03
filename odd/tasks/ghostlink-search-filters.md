# Ghostlink Search Filters (WU-022)

## Objective and scope
Make `search-inventory` (CLI and MCP) filter owned items by definition attributes: fix definition filters (`--type`, `--text`, `--tag`) being ignored for instance hits, and add damage type (`--element`), slot (`--slot`), class (`--class`) and minimum power (`--min-power`) filters.

Follow-up work units (out of scope here): WU-023 perk names (component 305 sockets + plug names), WU-024 item stats (component 304 + stat names).

## Decisions
- Damage type comes from the Manifest definition (`defaultDamageType`), not the item instance. Revisit if a real case needs the instance value.
- `--slot` and `--class` take human names (e.g. `Kinetic`, `Hunter`), not hashes.
- `--min-power` uses the already stored instance power; items without power never match a power filter.
- Definition filters constrain instance hits by resolving each instance's definition before truncation (no 64-hash intersection workaround).
- Schema migration v5 adds the new definition columns; existing databases need `ghostlink sync-manifest` to repopulate them.

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Fake and SQLite repositories keep identical matching rules.
- CLI and MCP stay in step; MCP schema changes are explicit (`deny_unknown_fields`).
- Checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Delivery: work-unit commits on `feature/ghostlink-search-filters`; push/PR/merge are user decisions.

## Tasks
- [x] **GSF-001 — Establish tracker and branch.** Branch `feature/ghostlink-search-filters` from `main` at `0363aaa`.
- [x] **GSF-002 — Core criteria and matching.** Add `element`, `slot`, `class_type` definition criteria and `min_power`; an instance query that carries definition filters; shared matching rule with unit tests and fake parity (including the `--type` regression). Delegated writer, test-first (RED: 21 missing-API errors in core tests). Typed `DamageType`/`ItemSlot`/`ClassType` with case-insensitive parsing; definition criteria boxed in the instance query; fake resolves each instance's definition before truncation. Constructor-only fixes across app/adapters/cli/mcp (new fields `None` until GSF-003). Checks: fmt, workspace all-features 997 passed/23 suites, clippy default and all-features, diff check; known OAuth timeout flake seen once. Commit evidence: see git log.
- [x] **GSF-003 — Manifest fields and schema v5.** Parse `classType`, `defaultDamageType` and bucket category from `DestinyInventoryItemDefinition`, map to names, migration v5, SQLite read/write paths and tests. Delegated writer (ports.rs surface added by parent), test-first (RED: missing candidate fields, round-trip loss, migration UnsupportedOperation). Decodes `classType`, `defaultDamageType`, bucket->slot; unknown values map to None. Reversible v5 adds nullable constrained lowercase columns and clears Manifest metadata so the next `sync-manifest` republishes even at the same live version (Manifest revision resets; profile data intact). Checks: fmt, workspace all-features 1,002 passed/24 suites, clippy both variants, diff check. Commit evidence: see git log.
- [x] **GSF-004 — Query service and SQLite instance search.** Definition filters and power constrain instance hits in `QueryService::search_inventory` and the SQLite instance search; any definition filter builds an instance query. Delegated writer, test-first (RED: SQLite returned nonmatching rows; two service regressions failed). SQLite instance search applies the shared Rust matching rule before truncation (resolves review advisory R3-sqlite-instance-parity); QueryService propagates definition filters and builds an instance query when needed. Parent fixed one stale MCP test that encoded the `--type` bug (`destiny-mcp/src/service.rs`, expected hits now `["inst-a"]`). Independent verifier: fmt, workspace all-features 1,006 passed/24 suites, clippy both variants, diff check. Commit evidence: see git log.
- [ ] **GSF-005 — CLI and MCP surfaces.** `--element`, `--slot`, `--class`, `--min-power` flags and matching MCP fields with schema tests; help and README.
- [ ] **GSF-006 — Verification and live validation.** Full checks, live `sync-manifest` + filtered searches, close tracker.

## Progress
Exploration (read-only): definition and instance queries run independently (`destiny-app/src/queries.rs:158-200`, `destiny-cli/src/command.rs:288-313`, `destiny-mcp/src/service.rs:634-679`); `element` hardcoded `None` in `destiny-adapters/src/manifest.rs:~602`; schema at v4 (`sqlite/migrations.rs:46`).

## Next step
GSF-005.
