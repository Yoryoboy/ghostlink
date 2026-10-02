# Ghostlink Live Manifest (WU-018)

## Objective and scope
Let the user populate live Destiny 2 Manifest item definitions with an explicit `ghostlink sync-manifest` command so `search-inventory` and `inspect-item` resolve real item hashes. Reuse the existing `BungieManifestGateway` -> `ManifestService` -> `SqliteStore::replace_definitions` pipeline, which is complete but unwired and not sized for live payloads.

Non-goals: activity, milestone, modifier, and perk tables (a later weekly-activities WU); languages other than English; automatic download on read; an MCP sync tool; changing `DESTINY_DEFINITION_VERSION` semantics (remains a label).

## Live measurements (2026-10-02, public endpoints, no credentials)
- Manifest version `244213.26.06.29.2000-1-bnet.65864`; index 189 KB.
- `DestinyInventoryItemDefinition` (en): 199,128,965 bytes uncompressed, 10,682,162 bytes gzip on the wire (server honors `Accept-Encoding: gzip`), 38,894 records, 2,207 with blank `displayProperties.name`, 7 redacted, max name length 91, every record has `inventory`.
- Other tables for later: Activity 11 MB, SandboxPerk 2.5 MB, ActivityModifier 206 KB, Milestone 38 KB.

## Gaps found (read-only exploration)
- Component cap 64 MiB and 15 s total timeout (`crates/destiny-adapters/src/manifest.rs`), no gzip feature on `reqwest`: the live items table cannot be acquired.
- `decode_inventory_items` parses the whole body as one `serde_json::Value` (very memory-heavy at ~200 MB).
- A blank name rejects the whole batch (decoder and `normalize_required_presentation`); live data has 2,207 such records.
- No production caller of the gateway or `ManifestService`; no CLI command; empty definitions make `search-inventory` silently return 0.

## Constraints and acceptance
- Hardening stays: endpoint/origin allowlist, no redirects/proxy, bounded retry, explicit byte caps (compressed and decompressed), sanitized errors, no API key on content requests.
- Typed, field-limited deserialization from the response; skip blank-name records (count them) instead of rejecting the batch.
- Re-running `sync-manifest` with an unchanged live version reports `unchanged` without re-downloading the component.
- `search-inventory` (CLI and MCP) on empty definitions fails with a clear, sanitized "definitions not loaded; run sync-manifest" outcome instead of 0 results.
- Test-first for every behavior task (RED, then GREEN). Checks per task: `cargo fmt --all -- --check`, `cargo build -p destiny-cli`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Worktree `/home/yoryo/apps/ghostlink-manifest` (own `target/`), so the WU-017 binary in the main worktree is untouched.
- Delivery: work-unit commits on `feature/ghostlink-live-manifest`; push/PR/merge are user decisions.

## Tasks
- [x] **LMF-001 — Establish tracker and branch.** Worktree branch `feature/ghostlink-live-manifest` from `main` at `382c4f5`. Commit evidence: `4899705`.
- [x] **LMF-002 — Live-sized acquisition.** gzip, separate compressed/decompressed caps and a longer component timeout, typed streaming decode of only the used fields, skip and count blank-name records. Delegated writer, test-first (RED: mixed blank/redacted batch failed with ProviderValidation). `reqwest` `gzip` feature (parent ran `cargo fetch`: `async-compression` 0.4.41, `compression-codecs`, `compression-core`); `flate2` dev-only. Component: 512 MiB decompressed cap counted while reading, 180 s timeout; index keeps 1 MiB and the short timeout. Only the decompressed cap is enforced (reqwest decodes transparently); it bounds memory. Typed streaming map visitor, no whole-body `Value`. Blank/redacted records skipped and counted (`skipped` on the acquisition); all-skipped batch is an error; 256-char name limit kept (live max 91). Parent fix (inline): writer had redacted the public URL in `ManifestHttpRequest` Debug, breaking an existing test; restored (only the API key is redacted). Verifier: fmt, 827 passed/19 suites, clippy, diff check. Commit evidence: `8204107`.
- [x] **LMF-003 — Version-aware publish.** Skip the component download when the stored definition version equals the live index version; report updated/unchanged, record count, skipped count; keep the atomic replace (batch the inserts if needed). Delegated writer, test-first (RED: missing sync/index APIs; GREEN). `ManifestGateway::fetch_index` + `load_component` (`load_definitions` kept as composition); `ManifestService::sync` -> `ManifestSyncReport::{Unchanged, Updated}` (version + counts only); `current_metadata()` distinguishes absence from storage failure; separate `DefinitionCatalogRepository` for counts; component version must match the index. Prepared statements reused; 40,000-row publish test passed in ~416 ms. Checks (writer): fmt, build, 833 passed/19 suites, clippy, diff check. Commit evidence: `f9911a6`.
- [ ] **LMF-004 — CLI command and empty-definitions outcome.** `ghostlink sync-manifest` (text + `--json`), runtime composition, and a clear sanitized outcome for `search-inventory` in CLI and MCP when definitions are empty.
- [ ] **LMF-005 — Live validation and docs.** Agent runs `sync-manifest` and `search-inventory` live (sanitized output); README section; close tracker.

## Progress
Exploration mapped the existing pipeline and gaps above; live sizes measured with public requests.

## Next step
LMF-004.
