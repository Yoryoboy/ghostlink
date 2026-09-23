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
- Minimal composition-foundation repair: `crates/destiny-core/src/{auth.rs,ports.rs,lib.rs}` for the complete credential contract; `crates/destiny-app/src/{auth.rs,config.rs,lib.rs}` for complete-credential use and an app-owned redacted shared read runtime configuration; `crates/destiny-adapters/src/{credential_store.rs,clock.rs,lib.rs}` for versioned secure persistence and `SystemClock`.
- MCP package: `crates/destiny-mcp/Cargo.toml`, `crates/destiny-mcp/src/main.rs`, and MCP-local modules under `crates/destiny-mcp/src/` for schemas/mapping, service routing, and the concrete composition root.
- MCP tests: in-module tests and/or `crates/destiny-mcp/tests/`; deterministic fakes may drive tests but never production behavior.
- User/run documentation only where behavior becomes runnable: `README.md` and this tracker.
- No planned edits to SQLite migrations, provider fixtures, query behavior, or CLI.

## Authorized composition repair

The user explicitly authorized the minimum cross-WU repair needed to make the stdio binary real rather than fixture-backed:

- `CredentialStore` persists and loads the complete `AccessCredential` (expiry metadata plus opaque material), never only the bearer bytes.
- Native keyring encoding is versioned and bounded. A legacy raw-material entry cannot recover expiry, so it is recognized without rendering or guessing, rejected as `ReauthorizationRequired`, and removed or replaced only through the explicit authorization lifecycle. There is no environment-derived expiry and no plaintext fallback.
- Application-owned shared read runtime configuration exposes only typed values needed by the trusted CLI/MCP read composition roots (API key wrapper input, database path, definition version, freshness TTL); its `Debug`, `Display`, and errors remain value-redacted.
- A production `SystemClock` implements the shared clock port using Unix seconds with sanitized pre-epoch/overflow failure.
- Only the `destiny-mcp` binary composition root names concrete adapters. Tool handlers and MCP library contracts continue to depend on the app-neutral `InventoryReadService`; adapter types never enter schemas/results/errors.
- Tests must prove new/legacy/malformed keyring behavior, exact complete-credential round trips, secret-free `Debug`/errors/log-facing values, redacted configuration, clock behavior, and that no fixture or environment expiry enters the production composition.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 2,400–4,000 authored changed lines across mapping/contracts, the authorized composition repair, server routing/runtime composition, stdio integration harness, snapshots, and documentation.
- Chain strategy: `feature-branch-chain`, continuing from WU-007 boundary `2b6c001`.
- Branch: `feature/destiny-mcp-read-tools`.
- Engram mirror topic: `odd/destiny-mcp-read-tools/tasks`.
- Review slices: tracker; bounded mapping/service contract; composition-foundation repair; stdio runtime/integration; final verification/closure.
- Every implementation slice includes its own tests/docs and closes with a Conventional Commit. No push or PR is part of this work unit.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`; workspace clippy across all targets with and without `--all-features` using `-D warnings`; workspace locked tests with and without all features; CLI/MCP help; `git diff --check`.
- Focused: MCP crate tests; exact tool names/descriptions/input schemas; argument bounds; list/search/inspect mapping parity; missing/ambiguous/error mapping; deterministic structured content; secret sentinel absence; stdout protocol discipline.
- Runtime harness: spawn the built `destiny-mcp` binary as a child, complete MCP initialize/list-tools/call-tool/shutdown over stdio, and exercise all three tools against deterministic offline services. No live Bungie account, network, credentials, or WU-008 binary is used.

## Tasks

- [x] **DMR-001 — Establish WU-011 boundary and pin official RMCP decisions**
  - Outcome: create this tracker and Engram mirror before source edits; record exact dependencies, edit surfaces, bounds, transport evidence, and the initial four implementation/closure tasks, checks, and rollback boundaries. The later authorized composition repair adds DMR-002A explicitly rather than hiding it inside DMR-003.
  - Check: tracker/Engram readback; clean `git diff --check`; commit contains only the tracker.
  - Rollback: remove only this tracker/branch before implementation.
  - Evidence: commit `3433bfa` (`chore: start Destiny MCP read tools`) contains only `odd/tasks/destiny-mcp-read-tools.md` (114 insertions, 0 deletions, confirmed by `git show --stat 3433bfa`); `git diff --check` was clean at that commit and the worktree was clean on `feature/destiny-mcp-read-tools` before DMR-002.

- [x] **DMR-002 — Add bounded MCP schema, translation, and service contract**
  - Outcome: pin RMCP; add MCP-owned inputs/outputs and a narrow application-service seam; map all three reads to shared services with deterministic bounded results and sanitized typed errors; prove mapping parity without transport.
  - Check: focused MCP unit/contract tests; fmt; focused clippy; schema/result snapshots or exact structure assertions; secret sentinel scan.
  - Rollback: remove the MCP dependency and local schema/mapping/service modules/tests while leaving the placeholder binary intact.
  - Evidence: commit `16b7809` (`feat: add bounded MCP read contracts`) pinned `rmcp = { version = "=3.4.0", features = ["server", "transport-io"] }` plus `serde`/`schemars`; added MCP-local `src/lib.rs`, `src/schema.rs`, and `src/service.rs`; added documented provider-neutral reexports through `destiny-app`; and applied `MAX_IDENTIFIER_LENGTH = 512` identically in JSON Schema/runtime without changing shared opaque IDs. Twenty focused MCP tests cover argument/identifier bounds, tool schemas, deterministic mapping, missing/ambiguous/sanitized errors, identity separation, secret absence, and shared-result parity. Independent verification closed both medium findings with no blocker/high/medium issue remaining. DMR-003/004 remain unchecked (DMR-002A was completed later).

- [x] **DMR-002A — Repair the minimum production composition foundations**
  - Outcome: persist/load a versioned complete `AccessCredential` through the native secure store; fail closed with explicit reauthorization for legacy entries whose expiry is unknowable; add redacted app-owned shared read runtime config and a production `SystemClock`; keep secrets and adapter types outside MCP/application result contracts.
  - Check: core/app/adapter focused tests; complete credential round trip; legacy and malformed-entry compatibility; no raw material in keyring diagnostics, `Debug`, errors, or log-facing values; config redaction; deterministic clock edge tests; fmt/clippy/diff checks.
  - Rollback: revert the complete-credential port/auth changes, remove versioned native-store encoding and `SystemClock`, and remove the shared read runtime-config surface; DMR-002 contracts remain usable with injected services and no stdio composition.
  - Evidence: authorized by the user after DMR-002 when repository inspection proved the old store discarded `expires_at` and could not construct `BungieProfileGateway` correctly in a new process. No environment expiry or fixture-backed production mode is permitted. Implemented with parent-owned commit identity in commit `7dc4984` (`feat: add shared read runtime foundations`): `CredentialStore` persists/loads the complete `AccessCredential` and `AuthorizationService` saves the whole credential instead of only its material (`crates/destiny-core/src/ports.rs`, `crates/destiny-app/src/auth.rs`, `crates/destiny-app/src/test_support.rs`); `NativeCredentialStore` writes a bounded versioned record (magic `GLCR`, version 1, big-endian expiry, four-byte material length) and fails a markerless legacy entry with `ReauthorizationRequired` and a corrupted/oversized record with `CredentialStoreUnavailable`, with no plaintext fallback (`crates/destiny-adapters/src/credential_store.rs`); the production `SystemClock` reports whole Unix seconds and fails pre-epoch/overflow with sanitized `ClockOverflow` (`crates/destiny-adapters/src/clock.rs`, `crates/destiny-adapters/src/lib.rs`); and `destiny-app` exposes the value-redacted shared read runtime config `ReadApiKey`/`ReadRuntimeConfig` (API key, database path, definition version, positive TTL bounded at `MAX_READ_FRESHNESS_TTL_SECONDS = 2592000` s), named for CLI/MCP reuse rather than any single binary, with `DESTINY_DATABASE_PATH`, `DESTINY_DEFINITION_VERSION`, and `DESTINY_FRESHNESS_TTL_SECONDS` documented in `config.example.env` (`crates/destiny-app/src/config.rs`, `crates/destiny-app/src/lib.rs`). A follow-up naming correction generalized the config surface from MCP-specific identifiers to shared read identifiers so both composition roots can consume it; no semantics or scope changed. Independent verifier lows 1, 3, and 4 were then applied: legacy classification now precedes versioned-record size validation (with an oversized markerless regression test), both `from_env` loaders now use a non-panicking exact-key `var_os` loader that rejects invalid Unicode sanitized and never reads unrelated variables, and the shared read definition version now rejects whitespace and control characters within the length bound. Verifier low 2 is accepted as an inherent fail-closed magic collision and recorded in the DMR-002A decisions below.

- [x] **DMR-003 — Run the tools through the stdio MCP transport**
  - Outcome: wire the RMCP tools-only server and binary lifecycle; prove initialize, discovery, and all three calls through a real child-process stdio session using deterministic offline composition; document invocation and protocol-only stdout.
  - Check: focused stdio integration suite, MCP help, offline runtime scenario, fmt/clippy/diff checks.
  - Rollback: remove stdio composition/integration harness and restore the contract-only MCP library from DMR-002.
  - Evidence: implemented on the committed DMR-002A foundation `7dc4984` plus the current uncommitted tracker evidence (parent-owned commit identity pending). Added `crates/destiny-mcp/src/server.rs` (`ReadToolServer` holds `Box<dyn InventoryReadService + Send>` behind a `std::sync::Mutex`, exposes exactly `tool_specs()`, dispatches each bounded `tools/call` through the existing `into_request()` conversions, offloads the synchronous read to `tokio::task::spawn_blocking`, and returns structured results or sanitized tool errors; unknown tool and malformed arguments are fixed protocol errors), `crates/destiny-mcp/src/runtime.rs` (`serve_stdio` over `rmcp::transport::stdio()` with a clean `waiting()` shutdown), the `destiny-mcp` binary composition root in `crates/destiny-mcp/src/main.rs` (`ReadRuntimeConfig`; `NativeCredentialStore` load with missing → `AuthorizationRequired` and legacy → propagated `ReauthorizationRequired`; `ProfileApiKey`; `HttpProfileTransport`; `SystemClock`; `BungieProfileGateway`; `SqliteStore`; `ProfileSyncService`/`QueryService`/`QueryReadService`), the test-only `offline-fixtures`-gated `crates/destiny-mcp/src/bin/offline_fixture.rs`, and `crates/destiny-mcp/tests/stdio.rs`. `crates/destiny-app/src/lib.rs` gained additive `AccessCredential`/`AccessCredentialMetadata`/`CredentialMaterial`/`CredentialStore` reexports so the root avoids a direct `destiny-core` dependency. `destiny-adapters` is a dependency of the binary crate only and is enforced by a library boundary test; the product binary has no fixture path or flag. README run/config docs updated. Exact commands and counts are in the DMR-003 verification table below.

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
- Existing raw-material-only native keyring entries produce an explicit reauthorization path; no expiry is guessed and no secret is migrated through plaintext or environment configuration.

## Transport evidence and open uncertainty

- Official source: <https://github.com/modelcontextprotocol/rust-sdk> and <https://docs.rs/rmcp/3.4.0/rmcp/>.
- Registry evidence on 2026-09-23: `cargo info rmcp --registry crates-io` reports official `rmcp` 3.4.0, Apache-2.0, MSRV 1.88, default `server`/`macros` support, `transport-io` for server stdio, and `transport-child-process` for client subprocess tests.
- Workspace evidence: Rust 1.98.1 and Cargo 1.98.1 satisfy the SDK MSRV.
- Remaining implementation uncertainty is mechanical API shape only: exact macro-generated router signatures and graceful child-process shutdown behavior must be proven against the pinned crate by compilation/runtime tests. It does not require a product decision and stays isolated inside `destiny-mcp`.

## Progress

- WU-007 is closed at base `2b6c001`; the worktree began clean on `feature/destiny-mcp-read-tools`.
- Repository mapping confirms `QueryService` already owns list/search/inspect freshness behavior and bounded provider-neutral results.
- DMR-001 is complete at commit `3433bfa`. DMR-002 is complete as a contract-only slice: pinned RMCP, bounded MCP schemas, injectable read-service seam, and translation parity tests. DMR-002A repaired the production composition foundations. DMR-003 is complete: the RMCP tools-only server runs over the stdio transport, the product `destiny-mcp` binary composes the concrete adapters, and a real offline child-process stdio test proves initialize/discovery/all three reads/cancellation/shutdown. DMR-004 (verify and close) remains unchecked.

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

### DMR-002A (authorized composition repair; committed at `7dc4984`, parent-owned)

Committed surfaces (parent-owned commit `7dc4984`, `feat: add shared read runtime foundations`): `crates/destiny-core/src/ports.rs`, `crates/destiny-app/src/{auth.rs,config.rs,lib.rs,test_support.rs}`, `crates/destiny-adapters/src/{credential_store.rs,clock.rs,lib.rs}`, and `config.example.env`. No `destiny-mcp` source, stdio/router/binary composition, SQLite/provider/query behavior, or fixture-backed production mode was added.

| Command | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | Clean after `cargo fmt --all` |
| `cargo check --workspace --all-targets --all-features` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | No issues found |
| `cargo clippy --workspace --all-targets -- -D warnings` | No issues found |
| `cargo test --workspace` | 406 passed, 0 failed (12 suites) |
| `cargo test --workspace --all-features` | 406 passed, 0 failed (12 suites) |
| `cargo test -p destiny-core --all-features` | 35 passed, 0 failed |
| `cargo test -p destiny-app --all-features` | 122 passed, 0 failed |
| `cargo test -p destiny-adapters --all-features` | 229 passed, 0 failed (4 suites) |
| `cargo test -p destiny-adapters --all-features credential_store` | 16 passed, 0 failed |
| `cargo test -p destiny-adapters --all-features clock::` | 6 passed, 0 failed |
| `cargo run -p destiny-cli -- --help` / `cargo run -p destiny-mcp -- --help` | Both succeeded |
| `git diff --check` | Clean |

Focused new tests: `credential_store::tests::{save_load_delete_round_trips_material_and_expiry_exactly, arbitrary_binary_material_round_trips_byte_exact, stored_record_is_versioned_and_never_the_plaintext_material, oversized_material_is_refused_before_it_is_written, legacy_raw_material_entry_requires_reauthorization_without_leaking_bytes, legacy_binary_entry_is_recognized_without_guessing_expiry, oversized_legacy_markerless_entry_is_still_legacy_and_fails_closed, malformed_marker_record_fails_sanitized, oversized_stored_record_fails_sanitized_without_rendering_bytes, formatted_errors_never_expose_secret_bytes, record_bounds_are_consistent}`; `clock::tests::{reports_whole_seconds_and_discards_subsecond_precision, the_epoch_itself_is_the_zero_boundary, pre_epoch_instants_fail_sanitized, negative_and_overflowing_seconds_fail_sanitized, the_u64_boundary_is_inclusive}`; `config::tests::{read_runtime_config_parses_every_typed_field, read_runtime_config_requires_every_value, read_runtime_config_rejects_invalid_values, read_runtime_config_debug_and_display_are_value_redacted, read_runtime_config_errors_do_not_echo_supplied_values, exact_key_values_reads_only_requested_keys_and_omits_absent_ones, exact_key_values_rejects_invalid_unicode_sanitized, read_definition_version_rejects_whitespace_and_control_characters}`; `auth::tests::behavioral::{authorization_persists_the_complete_credential_metadata_and_material}`; and `test_support::tests::in_memory_credential_store_round_trips_complete_credentials_and_redacts_material`.

DMR-002A decisions:

- A markerless stored value of any length is treated as a legacy raw-material entry: the marker test runs before versioned-record size validation, so an oversized raw value is still recognized as legacy (`ReauthorizationRequired`) rather than misreported as a storage failure. A value carrying the `GLCR`/version-1 marker but truncated, version-mismatched, length-inconsistent, or oversized is a corrupted record (→ sanitized `CredentialStoreUnavailable`). Neither path renders, echoes, or guesses the stored bytes.
- Accepted low-2 (inherent, fail-closed): a legacy raw token whose first four bytes happen to equal the `GLCR` marker is classified as a corrupted versioned record (`CredentialStoreUnavailable`) rather than legacy (`ReauthorizationRequired`). Both paths fail closed without rendering or guessing bytes; the ambiguity is inherent to prefix-based versioning and is accepted rather than papered over with a second marker or a different error.
- The record is bounded at `MAX_MATERIAL_LENGTH = 4096` bytes with an exact length check, so an oversized save is refused before anything is written and an oversized stored record fails closed without being decoded.
- `ReadRuntimeConfig` is a distinct app-owned surface rather than an extension of the existing `pub(crate)` `Config`: it validates only the four typed values the shared CLI/MCP read composition roots need and keeps them private behind trusted accessors, with `Debug`/`Display` fully value-redacted. The freshness TTL must be a positive decimal at or below 30 days; the definition version is provider-safe and must be non-empty, within the durable 128-character bound, and free of leading/trailing or interior whitespace and of control characters; the database path rejects empty, NUL-bearing, and surrounding-whitespace values; the API key reuses the visible-ASCII adapter rule. Names and internal env-key constants are deliberately not MCP-specific so the foundation stays selectively reusable by both roots; only binary wiring may name MCP.
- `Config::from_env` and `ReadRuntimeConfig::from_env` load exactly their required keys through a shared `var_os`-based exact-key loader; unrelated process variables are never inspected or retained, and a required value that is not valid Unicode fails closed as sanitized `ConfigurationError` instead of panicking. The loader core is unit-tested with an injected lookup (proving no extra key is queried) and, on Unix, an invalid-Unicode `OsString`.

### DMR-003 (stdio server and production composition; commit identity parent-owned)

Changed surfaces: `Cargo.toml` (added `destiny-adapters` and `tokio` workspace dependencies), `Cargo.lock` (additive), `crates/destiny-mcp/Cargo.toml` (`autobins = false`, `default-run = "destiny-mcp"`, non-default `offline-fixtures` feature, `destiny-adapters`/`tokio` dependencies, and explicit `destiny-mcp` and `offline-fixture` binaries with `required-features`), `crates/destiny-mcp/src/lib.rs`, new `crates/destiny-mcp/src/server.rs`, new `crates/destiny-mcp/src/runtime.rs`, `crates/destiny-mcp/src/main.rs`, new `crates/destiny-mcp/src/bin/offline_fixture.rs`, new `crates/destiny-mcp/tests/stdio.rs`, additive `AccessCredential`/`AccessCredentialMetadata`/`CredentialMaterial`/`CredentialStore` reexports in `crates/destiny-app/src/lib.rs`, and `README.md`. No core, adapter, or application behavior changed beyond the additive app reexports; no HTTP MCP transport, mutation, or WU-008 path was added.

| Command | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | Clean (`FMT_OK`) |
| `cargo check --workspace --all-targets --all-features` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | No issues found |
| `cargo clippy --workspace --all-targets -- -D warnings` | No issues found |
| `cargo test -p destiny-mcp` | 39 passed, 0 failed (4 suites; fixture and its integration test are gated off) |
| `cargo test -p destiny-mcp --all-features` | 40 passed, 0 failed (5 suites) |
| `cargo test -p destiny-mcp --all-features --test stdio` | 1 passed, 0 failed (1 suite) |
| `cargo test --workspace` | 425 passed, 0 failed (13 suites) |
| `cargo test --workspace --all-features` | 426 passed, 0 failed (14 suites) |
| `cargo run -p destiny-cli -- --help` / `cargo run -p destiny-mcp -- --help` | Both succeeded |
| Empty-environment `destiny-mcp` run | exit 1, empty stdout, stderr `required runtime configuration is missing or invalid` |
| `git diff --check` | Clean |

Focused new tests: `server::tests::{discovery_exposes_exactly_the_three_reads_in_stable_order, info_advertises_the_tools_capability_and_server_identity, get_tool_returns_only_the_discovered_tools, unknown_tool_is_a_fixed_protocol_error, malformed_arguments_are_a_fixed_protocol_error, each_tool_dispatches_bounded_arguments_to_the_shared_service, bound_violations_return_a_sanitized_tool_error, service_failures_return_a_sanitized_tool_error_without_raw_diagnostics, concurrent_dispatch_is_serialized_by_the_shared_service, repeated_dispatch_is_deterministic}`; library boundary tests `tests::{the_library_never_names_concrete_adapters, the_binary_root_owns_adapter_composition, the_product_binary_has_no_fixture_path_or_flag}`; binary composition tests `tests::{missing_credential_fails_closed_without_touching_the_environment, present_credential_is_returned_whole, legacy_credential_propagates_reauthorization_required, unavailable_store_propagates_its_sanitized_category, composition_fails_before_any_adapter_work_when_the_credential_is_missing, composition_fails_before_any_adapter_work_for_a_legacy_credential}`; and child-process integration `child_process_stdio_completes_initialize_discovery_calls_and_shutdown`.

DMR-003 decisions:

- The server holds `Box<dyn InventoryReadService + Send>` behind a `std::sync::Mutex` and runs the synchronous read through `tokio::task::spawn_blocking`. The lock is never held across an `.await`, and the blocking provider/storage work stays off the async RMCP loop; this is what makes the synchronous read composition safe inside the `Send + Sync + 'static` `ServerHandler`.
- Dispatch reuses the DMR-002 argument conversions. An unknown tool name and arguments that do not fit the bounded schema are fixed protocol errors (`-32601`/`-32602` with a constant message); a rejected bound or a service failure is a structured tool result with `isError: true` and the sanitized `McpError` code/message. No serde diagnostic, provider payload, or credential is echoed.
- `destiny-adapters` is a dependency of the `destiny-mcp` package for the binary composition root only; Cargo has no way to scope a dependency to a single binary target, so the boundary is enforced by a source test that the library, schema, server, and runtime modules never name `destiny_adapters`, and a test that the product `src/main.rs` has no `test_support`/`offline_fixture`/`offline-fixtures` path.
- The `offline-fixtures` feature only enables `destiny-app/test-support`; it does not modify the product binary. The separately named `offline-fixture` binary is a distinct, `required-features`-gated target and is unreachable from `destiny-mcp`.
- The child-process test hand-rolls newline-delimited JSON-RPC so it owns the raw stdout stream: every frame must parse as JSON-RPC (`jsonrpc: "2.0"`), the fixture sentinel and membership identifier are rejected on any stdout or stderr byte, and the run completes initialize, `tools/list`, all three `tools/call`s, a protocol error, a sanitized tool error, a cancellation notification, and a bounded clean shutdown.
- Product composition loads the credential before building any transport or database; a missing credential is `AuthorizationRequired` and a legacy entry propagates `ReauthorizationRequired` without a live keyring, network, or database in the failure-path tests.

DMR-003 rollback: revert the MCP crate to the DMR-002 contract-only library, remove the stdio server/runtime, fixture binary, and integration test, remove the `destiny-adapters`/`tokio` manifest entries (and refresh `Cargo.lock`), and revert the additive app reexports and README text; DMR-002A foundations and all completed WU-007 behavior stay intact.

## Closure

- DMR-004 (verify and close WU-011) is pending; DMR-003 is complete with the checks above and no commit has been created (parent-owned commit identity).
