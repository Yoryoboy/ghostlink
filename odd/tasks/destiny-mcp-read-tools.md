# Destiny MCP Read Tools

## Objective

Implement WU-011 from `docs/mvp-implementation-plan.md`: a thin stdio MCP server exposing `list_characters`, `search_inventory`, and `inspect_item` over the completed WU-007 application query services, with bounded schemas, sanitized results/errors, and no dependency on WU-008.

## Problem

WU-007 provides freshness-aware, adapter-neutral read services, but `destiny-mcp` is still only a command/configuration placeholder. MCP clients cannot discover or invoke the shared reads, and no transport-level contract proves that JSON-RPC requests remain bounded and secret-free.

## Why

WU-011 is the independent MCP read vertical. It must translate MCP inputs and outputs without duplicating query policy, preserve definition-versus-instance identity, isolate RMCP churn inside `destiny-mcp`, and prove stdio interoperability offline.

## Scope

- Pin the official Rust MCP SDK (`rmcp`) and only the features needed for a tools-only stdio server and its test client.
- Add MCP-owned JSON/schema DTOs for `list_characters`, `search_inventory`, and `inspect_item`.
- Translate validated MCP arguments into the existing `QueryService`/core query inputs and translate shared sanitized results into bounded MCP structured results.
- Preserve existing freshness behavior, deterministic ordering, ambiguity/missing outcomes, definition-versus-instance identity, and typed error categories.
- Expose the three tools through a stdio RMCP server; stdout is protocol-only and diagnostics remain sanitized.
- Contract-test MCP mapping against the same CLI-neutral application results and integration-test initialize/list-tools/call-tool over an offline stdio child process with deterministic fixture services.
- Snapshot or exact-structure test stable tool schemas/results and scan all MCP values/diagnostics for secret sentinels and unbounded/raw provider material.

## Constraints and decisions

- WU-011 depends on WU-007 and the WU-001 binary baseline, not on WU-008.
- `destiny-mcp` calls application services only. It does not call Bungie, SQLite, or core/adapters directly, and its manifest must retain no direct `destiny-core` dependency.
- RMCP compatibility is isolated in the MCP crate; application and core contracts remain unchanged unless a demonstrable adapter-neutral gap blocks safe translation.
- The official SDK selected at planning time is exact `rmcp = 3.4.0` (Apache-2.0, MSRV 1.88), compatible with this workspace's Rust 1.98.1. Server runtime uses features `server` and `transport-io`; client-only child-process support is a dev/test feature. The lockfile pins the complete transitive graph.
- RMCP 3.4.0 documents `transport::stdio()`, `ServiceExt::serve`, tool router/handler macros, and `transport-child-process`. Implementation must verify these APIs by compilation and focused tests rather than spreading SDK types outside the presentation crate.
- Only stdio transport is in scope. No HTTP/SSE transport, resources, prompts, sampling, model-specific behavior, mutation, approval flow, or raw provider proxy is added.
- MCP results are finite and deterministic. Caller-provided search limits remain constrained by the shared maximum; text/tag/hash candidate inputs receive explicit adapter bounds before service invocation.
- Errors expose stable sanitized categories/messages only. Credentials, API keys, bearer headers, authorization codes, provider payloads, configuration values, and debug representations never enter tool content or JSON-RPC error data.
- Technical artifacts are English. Automated checks are offline and credential-free.
- TDD mode is not configured by project/session evidence; tests remain in the same commit as behavior.
- Native RDD is disabled for this clone by prior explicit user decision; ordinary risk-gated verification applies.

## Exact edit surfaces

- Workspace dependency/lock surface: `Cargo.toml`, `Cargo.lock`.
- MCP package: `crates/destiny-mcp/Cargo.toml`, `crates/destiny-mcp/src/main.rs`, and new MCP-local modules under `crates/destiny-mcp/src/` for schemas/mapping, service routing, and composition.
- MCP tests/fixtures only: in-module tests and/or `crates/destiny-mcp/tests/` plus deterministic sanitized fixtures if needed.
- User/run documentation only where behavior becomes runnable: `README.md` and this tracker.
- No planned edits to `destiny-core`, `destiny-app`, `destiny-adapters`, CLI, migrations, or provider fixtures.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 1,200–2,000 authored changed lines across mapping/contracts, server routing/runtime composition, stdio integration harness, snapshots, and documentation.
- Chain strategy: `feature-branch-chain`, continuing from WU-007 boundary `2b6c001`.
- Branch: `feature/destiny-mcp-read-tools`.
- Engram mirror topic: `odd/destiny-mcp-read-tools/tasks`.
- Review slices: tracker; bounded mapping/service contract; stdio runtime/integration; final verification/closure.
- Every implementation slice includes its own tests/docs and closes with a Conventional Commit. No push or PR is part of this work unit.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`; workspace clippy across all targets with and without `--all-features` using `-D warnings`; workspace locked tests with and without all features; CLI/MCP help; `git diff --check`.
- Focused: MCP crate tests; exact tool names/descriptions/input schemas; argument bounds; list/search/inspect mapping parity; missing/ambiguous/error mapping; deterministic structured content; secret sentinel absence; stdout protocol discipline.
- Runtime harness: spawn the built `destiny-mcp` binary as a child, complete MCP initialize/list-tools/call-tool/shutdown over stdio, and exercise all three tools against deterministic offline services. No live Bungie account, network, credentials, or WU-008 binary is used.

## Tasks

- [x] **DMR-001 — Establish WU-011 boundary and pin official RMCP decisions**
  - Outcome: create this tracker and Engram mirror before source edits; record exact dependencies, edit surfaces, bounds, transport evidence, four implementation/closure tasks, checks, and rollback boundaries.
  - Check: tracker/Engram readback; clean `git diff --check`; commit contains only the tracker.
  - Rollback: remove only this tracker/branch before implementation.
  - Evidence: commit `3433bfa` (`chore: start Destiny MCP read tools`) contains only `odd/tasks/destiny-mcp-read-tools.md` (114 insertions, 0 deletions, confirmed by `git show --stat 3433bfa`); `git diff --check` was clean at that commit and the worktree was clean on `feature/destiny-mcp-read-tools` before DMR-002.

- [x] **DMR-002 — Add bounded MCP schema, translation, and service contract**
  - Outcome: pin RMCP; add MCP-owned inputs/outputs and a narrow application-service seam; map all three reads to shared services with deterministic bounded results and sanitized typed errors; prove mapping parity without transport.
  - Check: focused MCP unit/contract tests; fmt; focused clippy; schema/result snapshots or exact structure assertions; secret sentinel scan.
  - Rollback: remove the MCP dependency and local schema/mapping/service modules/tests while leaving the placeholder binary intact.
  - Evidence: pinned `rmcp = { version = "=3.4.0", features = ["server", "transport-io"] }` (resolved to rmcp 3.4.0 in `Cargo.lock`) plus `serde`/`schemars` workspace dependencies; added MCP-local `src/lib.rs`, `src/schema.rs`, and `src/service.rs`; added a documented additive `destiny-core` reexport block to `crates/destiny-app/src/lib.rs` so the presentation crate can translate shared read types without a direct `destiny-core` dependency; added one MCP-local identifier transport bound (`MAX_IDENTIFIER_LENGTH = 512`, `character_id`/`instance_id`, empty rejected) enforced in JSON Schema and in runtime conversion before any service call; 20 focused MCP tests cover argument bounds, identifier bounds with zero-call behavior, tool/JSON-schema shape, deterministic mapping, missing/ambiguous/sanitized errors, definition-vs-instance identity, secret-sentinel absence, and parity with the shared application results. DMR-003/004 remain unchecked.

- [ ] **DMR-003 — Run the tools through the stdio MCP transport**
  - Outcome: wire the RMCP tools-only server and binary lifecycle; prove initialize, discovery, and all three calls through a real child-process stdio session using deterministic offline composition; document invocation and protocol-only stdout.
  - Check: focused stdio integration suite, MCP help, offline runtime scenario, fmt/clippy/diff checks.
  - Rollback: remove stdio composition/integration harness and restore the contract-only MCP library from DMR-002.

- [ ] **DMR-004 — Verify and close WU-011**
  - Outcome: run the full effective check set, review every changed file against WU-011 acceptance criteria, record exact commit/check evidence and known manual limitations, then close this tracker.
  - Check: complete baseline/focused/runtime commands, clean status, authored line count, and no blocker/high/medium finding.
  - Rollback: reset the feature branch to exact base `2b6c00159ebc574d1e97d9ffc490244d04f1ca70` to remove only WU-011.

## Acceptance criteria

- MCP clients discover exactly the semantic read tools `list_characters`, `search_inventory`, and `inspect_item` over stdio.
- Every tool delegates through shared application query behavior; no MCP-local freshness, search, storage, provider, or authorization policy is invented.
- MCP arguments and outputs are explicitly bounded, deterministic, and preserve definition hash versus concrete instance ID.
- Missing, ambiguity, freshness, and typed failures translate predictably without leaking secret or raw provider material.
- Contract tests demonstrate parity with CLI-neutral application results, and a real child-process stdio test completes discovery and all three read calls offline.
- `destiny-mcp` remains independent of WU-008 and has no direct dependency on `destiny-core` or provider adapters.
- Every completed task records exact checks, rollback boundary, and Conventional Commit identity.

## Transport evidence and open uncertainty

- Official source: <https://github.com/modelcontextprotocol/rust-sdk> and <https://docs.rs/rmcp/3.4.0/rmcp/>.
- Registry evidence on 2026-09-23: `cargo info rmcp --registry crates-io` reports official `rmcp` 3.4.0, Apache-2.0, MSRV 1.88, default `server`/`macros` support, `transport-io` for server stdio, and `transport-child-process` for client subprocess tests.
- Workspace evidence: Rust 1.98.1 and Cargo 1.98.1 satisfy the SDK MSRV.
- Remaining implementation uncertainty is mechanical API shape only: exact macro-generated router signatures and graceful child-process shutdown behavior must be proven against the pinned crate by compilation/runtime tests. It does not require a product decision and stays isolated inside `destiny-mcp`.

## Progress

- WU-007 is closed at base `2b6c001`; the worktree began clean on `feature/destiny-mcp-read-tools`.
- Repository mapping confirms `QueryService` already owns list/search/inspect freshness behavior and bounded provider-neutral results.
- DMR-001 is complete at commit `3433bfa`. DMR-002 is complete as a contract-only slice: pinned RMCP, bounded MCP schemas, injectable read-service seam, and translation parity tests. `destiny-mcp` still contains only the Clap placeholder binary; the stdio server and production composition remain DMR-003.

## Verification evidence

### DMR-001

- `git show --stat 3433bfa` / `git show --name-only 3433bfa`: sole path `odd/tasks/destiny-mcp-read-tools.md`, 114 insertions, 0 deletions.
- `git diff --check` at the DMR-002 start: clean; `git status --porcelain`: clean on `feature/destiny-mcp-read-tools`.

### DMR-002 (contract-only; no stdio runtime and no production composition)

- Dependency pin: `cargo tree -p destiny-mcp` resolves `rmcp v3.4.0`; `Cargo.lock` gained 354 additive lines with no removals, including `rmcp 3.4.0`, `schemars`, and the `tokio`/`futures`/`uuid` transport graph.
- Changed surfaces: `Cargo.toml`, `crates/destiny-mcp/Cargo.toml`, new `crates/destiny-mcp/src/lib.rs`, new `crates/destiny-mcp/src/schema.rs`, new `crates/destiny-mcp/src/service.rs`, additive reexports in `crates/destiny-app/src/lib.rs`, and new `Cargo.lock` entries. `crates/destiny-mcp/src/main.rs` stays the untouched Clap placeholder.
- Rollback boundary: base `3433bfa`. Removing the `rmcp`/`serde`/`schemars` manifest entries (and refreshing `Cargo.lock`), deleting the three new MCP library modules, and reverting the additive `destiny-app` reexport block returns the tree to DMR-001 without touching the placeholder binary or any completed WU-007 behavior.

| Command | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | Clean (`FMT_OK`) |
| `cargo test -p destiny-mcp` | 20 passed, 0 failed (3 suites) |
| `cargo test -p destiny-app --all-features` | 113 passed, 0 failed (2 suites); includes the presentation-crate no-`destiny-core` manifest check |
| `cargo clippy -p destiny-mcp -p destiny-app --all-targets --all-features -- -D warnings` | No issues found |
| `cargo check --workspace --all-targets --all-features` | Passed |
| `cargo test --workspace --all-features` | 383 passed, 0 failed (12 suites) |
| `git diff --check` | Clean |

Focused MCP tests: `schema::tests::{tool_specs_expose_only_the_three_semantic_reads_in_stable_order, search_inventory_schema_declares_the_shared_bounds, identifier_schemas_declare_the_mcp_transport_bound, list_and_inspect_schemas_are_closed_objects, unknown_arguments_are_rejected_by_every_input_type, omitted_limits_default_to_the_shared_maximum, error_codes_are_stable_and_messages_are_sanitized}` and `service::tests::{definition_bounds_are_rejected_before_any_service_call, instance_candidate_bounds_are_rejected_before_any_service_call, invalid_identifiers_are_rejected_before_any_service_call, identifier_boundary_is_inclusive, list_characters_parity_with_the_shared_application_result, search_inventory_parity_with_the_shared_application_result, inspect_item_parity_keeps_definition_and_instance_distinct, inspect_item_reports_missing_as_an_explicit_successful_outcome, ambiguous_instance_fails_closed_with_a_sanitized_error, provider_failures_surface_only_sanitized_categories, mcp_values_never_carry_raw_provider_material, translation_is_deterministic_across_repeated_reads, the_read_service_seam_is_object_safe_and_injectable}`.

DMR-002 decisions:

- `search_inventory` mirrors the shared request shape: the definition query always runs and an optional nested `instances` object carries the concrete-instance query; `item_hashes: null` means no constraint while `[]` stays fail-closed.
- `inspect_item` maps the shared `Resolution::Missing` to an explicit successful `Missing` outcome; ambiguity and invalid projection state stay sanitized `local_storage` errors, matching the shared fail-closed behavior.
- An opaque `ItemLocation::Other` value is rendered as `LocationView::Other` without its provider text.
- Identifier bounding is MCP-local transport policy, not a core rule: `MAX_IDENTIFIER_LENGTH = 512` characters bounds `character_id` and `instance_id`, and empty identifiers are rejected, because the shared opaque `CharacterId`/`ItemInstanceId` types accept any string. The bound is declared in the JSON Schema (`minLength: 1`, `maxLength: 512`) and enforced in runtime conversion before any service call; no core identifier type changed.
- All `rmcp` types (`Tool`, `CallToolResult`) stay in `crates/destiny-mcp/src/schema.rs`; the `InventoryReadService` seam and translation hold no SDK type and are object-safe.
- The additive `destiny-app` reexports are the documented transitive-public-type gap: the presentation crate needs the shared query/result/bounds/identity types without a direct `destiny-core` manifest dependency.

DMR-002 known limitations carried to DMR-003: stdio wiring, child-process shutdown, the macro-generated router (the default `macros` feature is already enabled), production service composition, and protocol-only stdout discipline are not implemented or proven here.

## Closure

- Pending.
