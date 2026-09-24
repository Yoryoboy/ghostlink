# Destiny Offline Fixture Journey (WU-014)

## Status and objective

IN PROGRESS on `feature/destiny-offline-fixture-journey`, stacked on WU-013 `feature/destiny-hardening` at `845be92`. Demonstrate the deterministic, credential-free journey from fixture authorization and Manifest/profile projection through search/inspect, semantic "The Last Word to Warlock" intent, one direct MCP move, fresh preflight, fake transfer, post-action refresh and concrete verification. Preserve truthful partial-failure evidence. No live Bungie compatibility claim or multi-call atomicity claim.

## Scope and constraints

- Reuse existing production application, adapter, CLI and MCP boundaries; test-only transport/clock/credential fakes are allowed. Do not add a CLI mutation command (CLI is currently read-only).
- Prove CLI/MCP parity for their shared read services through machine-readable results; MCP mutation remains the single-call authorization surface. Manual offline adapter runs use existing scaffold commands and documented fixture harnesses, not live credentials or OS-keyring success claims.
- No network, real account, persisted secrets, raw provider leakage, or dependency churn solely to introduce snapshots. Keep fixtures deterministic and sanitized; re-use existing snapshots/golden conventions if any.
- Conversation in Spanish; repository artifacts, test names, and commits in English. No push, PR, merge into default branch, or release without a new user decision.
- Configured TDD: no project/session mode found (prior WU-013 tracker and exploration); ordinary behavior checks apply. Exact baseline runner: `cargo test --workspace --all-features`; focused package/test filters allowed. Additional gates: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Delivery strategy: `ask-on-risk`; user chose `stacked-to-main` for this WU on 2026-09-24. Forecast roughly 500–700 authored lines (additions plus deletions, excluding generated snapshots if any), to be divided into cohesive local review slices without code golf. Running committed authored lines: 0. Slice boundaries and included commits to be recorded as work proceeds.

## Acceptance

- AC-3/5: a real adapter/storage fixture path produces a fresh, revisioned profile and searchable/inspectable concrete instance, with Manifest definitions supplied deterministically.
- AC-4/6/7: one explicit-intent MCP mutation call resolves the named item and Warlock, binds an internal plan, performs mandatory fresh preflight and the minimum transfer, refreshes, and verifies a concrete destination; stale/preflight-change and partial failure alternatives remain honest. CLI/MCP read results represent the same service state, not an imaginary CLI mutation parity.
- AC-9: repeatable offline full journey and stable, safe key-output assertions/snapshots; no live credentials/network or atomicity claim. Existing WU-013 secret-leak checks remain green.
- Report any impossible composition or missing production seam before broadening scope; no acceptance criterion is silently waived.

## Tasks

- [ ] **DFJ-001 — Establish branch and recovery tracker.** Route: inline mechanical tracker + branch state; mapping of 4+ files delegated to `gentle-ai-explore`. Checks: clean stacked base, local file and full Engram mirror readback, visible todo. Rollback: remove this tracker and the new branch only before further commits. Commit: pending.
- [ ] **DFJ-002 — Compose the full real-adapter offline fixture journey.** Route: delegate bounded writer (multi-file preparation/verification), focused source surface `crates/destiny-adapters/tests/fixture_journey.rs` and, only if necessary, a narrowly identified shared test helper. Reuse test-only provider transport, injected clock and real SQLite, profile/definition/search/inspect/planner/executor/transfer paths; include success, stale/preflight invalidation, and partial-failure cases. Checks: focused integration test, formatting, applicable clippy, offline runtime scenario; independent verification per native risk assessment. Rollback: remove only journey test/helper and its fixture-local dependencies. Commit: pending.
- [ ] **DFJ-003 — Prove CLI/MCP read parity and stable outputs.** Route: delegate bounded writer (2+ test/harness surfaces); candidate surfaces `crates/destiny-mcp/tests/stdio.rs`, `crates/destiny-mcp/src/bin/offline_fixture.rs`, `crates/destiny-adapters/tests/fixture_journey.rs`, and narrowly identified CLI test surface after mapping. Compare machine-readable CLI/MCP shared read semantics, pin safe deterministic key outputs without fragile platform snapshots, and assert one MCP direct move in its existing fixture harness. Checks: focused CLI/MCP tests and offline child-process runtime, format/clippy, full suite at closure. Rollback: remove parity/golden assertions and any test-only fixture wiring, preserving existing functionality. Commit: pending.
- [ ] **DFJ-004 — Reconcile WU-014 evidence and close local work unit.** Route: inline docs/tracker if mechanical, delegate verification commands to `gentle-ai-verify` when native risk plan requires. Document the exact fixture commands and limitations in `docs/mvp-implementation-plan.md` and this tracker; run all baseline gates and independent spot-check; record commit IDs, slice boundaries, failures/skips and next WU-015. Rollback: revert closure documentation only. Commit: pending.

## Progress and verification evidence

- Base checked clean at `845be92`; created local `feature/destiny-offline-fixture-journey` from WU-013.
- Read-only scout mapped existing separate planning, transfer, query, MCP stdio and leak journeys; none alone proves the complete real-adapter named journey. Existing CLI is read-only, and CLI success child-process remains blocked by production OS credential composition; preserve this limitation.
- Tests, runtime harnesses, and commits: pending.

## Next step

Complete DFJ-001 persistence/projection, then derive and dispatch DFJ-002 with concrete test interfaces and no invented live compatibility claims.
