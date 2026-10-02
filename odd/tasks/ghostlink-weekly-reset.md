# Ghostlink Weekly Reset (WU-019 public, WU-020 personal)

## Objective and scope
WU-019 (this branch): show what the current Destiny 2 weekly reset offers without credentials. Load the Activity, Milestone, and ActivityModifier Manifest tables through `sync-manifest`, read Bungie `GET /Destiny2/Milestones/` (GetPublicMilestones, API key only), resolve hashes against stored definitions, and expose the result as a `ghostlink weekly-reset` CLI command and a read-only MCP tool.

WU-020 (chained branch on top of WU-019): per-character weekly progress from profile components 202 (CharacterProgressions, milestones) and 204 (CharacterActivities), through a separate profile gateway method so the inventory request string stays unchanged.

Non-goals (WU-019): per-character progress (WU-020); SandboxPerk or other tables; languages other than English; Portal/activity-graph navigation; an MCP sync tool; caching milestones beyond their own reset window.

## Exploration findings
- `crates/destiny-adapters/src/manifest.rs` decodes only `DestinyInventoryItemDefinition`. `ManifestDownloadPlan` and the core `ManifestGateway` port (`crates/destiny-core/src/ports.rs:505-558`) are single-component and item-shaped.
- SQLite schema `LATEST_VERSION = 2`; `sync_metadata.dataset` CHECK allows only `manifest_definitions` and `profile_inventory`, so a v3 migration needs a table rebuild to add datasets.
- `is_allowed_manifest_url` gates the transport; GetPublicMilestones needs its own allowlisted endpoint and byte cap.
- CLI: credential-free `sync-manifest` path in `crates/destiny-cli/src/{command,runner,runtime,output}.rs` is the model. MCP: tool constants and specs in `crates/destiny-mcp/src/schema.rs`; exact tool lists asserted in `server.rs`, `schema.rs`, `main.rs`, and `tests/stdio.rs`.
- Live sizes (WU-018): Activity 11 MB, ActivityModifier 206 KB, Milestone 38 KB.

## Constraints and acceptance
- Hardening stays: endpoint allowlist, no redirects or proxy, bounded retry, explicit byte caps, typed field-limited decoding, sanitized errors, API key only on index and milestones requests.
- `sync-manifest` stays version-aware: an unchanged version downloads nothing; an updated version replaces all four tables atomically.
- `weekly-reset` reports each public milestone with resolved names, its activities and modifiers, and the reset window; unknown hashes are reported as unresolved, never guessed. Missing definitions return the existing `definitions not loaded; run ghostlink sync-manifest` outcome.
- Test-first for every behavior task (RED, then GREEN). Checks per task: `cargo fmt --all -- --check`, `cargo build -p destiny-cli`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Worktree `/home/yoryo/apps/ghostlink-weekly` (own `target/`), so the main-worktree binary used by the URI handler is untouched.
- Delivery: work-unit commits on `feature/ghostlink-weekly-reset`; push/PR/merge are user decisions.

## Tasks
- [ ] **WRS-001 — Live evidence and domain boundary.** Measure the live GetPublicMilestones payload (size, fields used) and the shape of the three new tables; define core domain types (activity, milestone, modifier definitions; public milestone snapshot) and ports.
- [ ] **WRS-002 — Multi-table Manifest acquisition.** Per-component download plan and typed decoders for Activity, Milestone, and ActivityModifier; version-aware sync covers all tables.
- [ ] **WRS-003 — SQLite v3 storage.** Migration with definition tables for the new kinds and the widened `sync_metadata` datasets; atomic publish and lookups.
- [ ] **WRS-004 — Public milestones gateway.** Allowlisted endpoint, byte cap, typed decode into the domain snapshot.
- [ ] **WRS-005 — Weekly reset service.** App service resolving milestone, activity, and modifier hashes; explicit unresolved outcomes; definitions-unavailable error.
- [ ] **WRS-006 — CLI `weekly-reset`.** Text and `--json` output, credential-free runtime composition.
- [ ] **WRS-007 — MCP read tool.** `weekly_reset` tool with schemas, dispatch, and updated tool-list assertions.
- [ ] **WRS-008 — Live validation, docs, and close.** Live run with sanitized output, README section, tracker closure.

## Progress
Branch `feature/ghostlink-weekly-reset` created in worktree `/home/yoryo/apps/ghostlink-weekly` from `main` at `083278a`.

## Next step
WRS-001.
