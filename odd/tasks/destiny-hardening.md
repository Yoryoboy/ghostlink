# Destiny Hardening

## Objective

Implement WU-013 from `docs/mvp-implementation-plan.md`: harden Destiny paths with shared redaction, bounded retry behavior, correlation/plan/step observability, fail-closed credential fallback policy, and automated leak checks across durable storage, CLI, and MCP boundaries.

## Problem

WU-003, WU-005, WU-008, WU-010, WU-011, and WU-012 exist as local work-unit branches, but the current WU-012 closure branch does not include the completed WU-008 CLI read surface. WU-013 depends on WU-008 and cannot honestly prove CLI leak checks until that local branch is integrated into this WU-013 branch. Existing redaction is mostly hand-rolled per adapter/type, there is no logging framework or diagnostic sink, and retry behavior is status-classification-only rather than a bounded policy.

## Why

Security hardening must be cross-boundary, not local to one adapter. Secrets, auth material, provider payloads, membership identifiers, and raw error details must stay out of `Display`/`Debug`, process streams, SQLite files/sidecars, MCP serialization, CLI output, and any future diagnostics. Transfer mutation behavior must also remain intent-bound and never gain unsafe blind retry semantics.

## Scope

- Integrate the completed local WU-008 CLI read journey into the WU-013 branch before hardening work.
- Add or consolidate shared redaction primitives for secret-bearing values and safe diagnostics.
- Preserve and expose existing `PlanId` and step evidence, and add only the minimum correlation identity needed for request-level diagnostics if required by the hardening design.
- Add bounded retry policy for transient/rate-limit read paths only, with explicit non-retry behavior for mutation execution steps.
- Add automated leak checks using shared secret sentinels across SQLite bytes/sidecars, CLI output, MCP stdio, serialized errors, and any diagnostics introduced by this work.
- Keep credential storage fail-closed: no plaintext, filesystem, or environment credential fallback is added for the MVP.
- Update project documentation to state the hardening contract and the deliberate no-fallback credential policy.

## Constraints and decisions

- User selected `Integrate WU-008 first` on 2026-09-24; WU-013 must be based on both WU-012 and the completed local WU-008 branch before hardening implementation proceeds.
- User selected `Fail-closed only` for credential fallback on 2026-09-24; do not add an opt-in plaintext/filesystem fallback gate for MVP.
- Technical artifacts are English.
- No live Bungie account, network, or real credentials are required by automated checks.
- Existing MCP mutation remains default-enabled and single-call intent-authorized; hardening must not weaken its explicit intent, preflight, evidence, or post-action verification contracts.
- There is no logging framework in the current codebase; if no diagnostics sink is introduced, WU-013 records and tests the current no-log/no-leak process-stream contract instead of inventing runtime logging.
- TDD mode: not configured by project or session evidence; ordinary functional checks apply.
- Native RDD: disabled for this clone by prior explicit user decision; ordinary risk-gated verification applies.

## Delivery

- Strategy: `ask-on-risk`.
- Chain strategy: `stacked-to-main`, continuing the local work-unit branch chain from WU-012 after integrating WU-008.
- Branch: `feature/destiny-hardening`.
- Start boundary: `b2ad2a9` (`docs: close Destiny MCP mutation tool`).
- Required dependency integration: local branch `feature/destiny-cli-read-journey` ending at `ae09804`.
- Engram mirror topic: `odd/destiny-hardening/tasks`.
- Review slices: (a) dependency integration plus redaction/retry behavior, then (b) cross-boundary leak tests and fail-closed credential fallback policy.
- Forecast: substantial and likely over the 400-line review heuristic because WU-008 integration and cross-boundary leak harnesses are cohesive; keep tests with behavior and record accepted over-budget work units rather than splitting artificially.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`; CLI/MCP help or stdio fixture checks where applicable; `git diff --check`.
- Focused integration: WU-008 CLI runner and query journey checks after dependency integration.
- Focused hardening: core/app/adapters redaction tests; retry policy contract tests; mutation non-retry tests; credential-store fail-closed tests; MCP stdio leak tests; consolidated SQLite/process stream sentinel harness.
- Runtime/leak harness: deterministic fixture journey that scans SQLite database bytes and sidecars, CLI stdout/stderr, MCP stdout/stderr, and serialized error shapes for secret sentinels, with at least one positive non-secret control.

## Tasks

- [x] **DHS-001 — Establish WU-013 branch, decisions, and hardening tracker**
  - Route: delegated read-only mapping because WU-013 spans more than four files; parent created the one-file ODD tracker before source writes.
  - Outcome: created `feature/destiny-hardening`, mapped redaction/logging/retry/fallback/leak-test surfaces, resolved the WU-008 dependency integration choice, and resolved the MVP credential fallback policy as fail-closed only.
  - Check: tracker readback passed; Engram mirror readback passed for observation `5995`; visible TODO projection created.
  - Rollback: delete only `odd/tasks/destiny-hardening.md` and the `feature/destiny-hardening` branch before source implementation.
  - Evidence: commit `d81d44f` (`chore: start Destiny hardening`).

- [x] **DHS-002 — Integrate the WU-008 CLI read dependency into the WU-013 branch**
  - Route: delegated multi-file writer because integration touches CLI, app/core exports, adapters journey tests, workspace manifests, and docs.
  - Allowed edit surfaces: `Cargo.toml`, `Cargo.lock`, `README.md`, `crates/destiny-cli/**`, `crates/destiny-app/src/{lib.rs,queries.rs,test_support.rs}`, `crates/destiny-core/src/{lib.rs,mutation.rs,ports.rs}`, `crates/destiny-adapters/{Cargo.toml,src/lib.rs,tests/query_journey.rs}`, `crates/destiny-mcp/Cargo.toml`, `odd/tasks/destiny-cli-read-journey.md`, and this tracker.
  - Outcome: integrated local `feature/destiny-cli-read-journey` (`ae09804`) into `feature/destiny-hardening` as an uncommitted working-tree integration. Added the WU-008 CLI read surface (`crates/destiny-cli/src/{command.rs,lib.rs,output.rs,runner.rs,runtime.rs}` plus the rewritten `main.rs` and the `destiny-adapters`/`serde_json` CLI dependencies), the `QueryOperations` read trait in `crates/destiny-app/src/queries.rs`, the CLI variant of `crates/destiny-adapters/tests/query_journey.rs`, the union re-export block in `crates/destiny-app/src/lib.rs`, the merged `README.md` CLI section, the merged `Cargo.lock`, the `destiny-cli` dev-dependency in `crates/destiny-adapters/Cargo.toml`, and the WU-008 tracker `odd/tasks/destiny-cli-read-journey.md`. The current WU-009/WU-010/WU-011/WU-012 mutation, transfer, MCP read, and MCP mutation files and trackers are retained unchanged; only the README, the app re-export block, and the tracker conflicted and were resolved as a union of both sides.
  - Checks: `cargo fmt --all -- --check` -> exit 0; `cargo check --workspace --all-targets --all-features` -> exit 0; focused `cargo test -p destiny-adapters --all-features --test query_journey` -> 14 passed, 0 failed; `cargo test --workspace --all-features` -> 672 passed, 0 failed (18 suites, includes MCP stdio and transfer/mutation journeys); `cargo clippy --workspace --all-targets --all-features -- -D warnings` -> no issues; `git diff --check` -> exit 0. Parent spot-check reran `cargo test -p destiny-adapters --all-features --test query_journey` -> 14 passed. Native assessment was unassessable because untracked files require explicit declaration, so independent verifier treated it as high risk and passed: `cargo test -p destiny-cli` -> 54 passed; CLI help exit 0; unset read config smoke exit 52 sanitized; `git diff --check` exit 0; no blocker/high/medium finding. LSP reported stale/false rust-analyzer diagnostics in the newly integrated CLI tests/patterns, contradicted by rustc, clippy, and the observed test suite.
  - Runtime harness: real CLI runner over the deterministic offline query journey, no live credentials/network. `cargo run -p destiny-cli -- --help` exits 0 and `cargo run -p destiny-cli -- list-characters` without read configuration exits 52 with the sanitized `required runtime configuration is missing or invalid` message.
  - Rollback: revert this integration commit to return to pure WU-012 closure before hardening.
  - Evidence: commit `b34a729` (`feat: integrate Destiny CLI reads`).

- [ ] **DHS-003 — Consolidate redaction and request correlation contracts**
  - Route: delegated multi-file writer because secret-bearing types span core, app, and adapters.
  - Allowed edit surfaces: `crates/destiny-core/src/{lib.rs,error.rs,ids.rs,ports.rs,auth.rs,redact.rs}`, `crates/destiny-app/src/{config.rs,auth.rs,test_support.rs}`, `crates/destiny-adapters/src/{token.rs,profile_gateway.rs,transfer.rs,credential_store.rs}`, and this tracker.
  - Outcome: added `crates/destiny-core/src/redact.rs` as the shared core redaction module: the single `REDACTED` marker, a generic `Redacted<T>` wrapper (redacted `Debug`/`Display`, explicit trusted `expose_secret`/`into_inner`), `redact_optional` for presence-preserving field redaction, and `opaque` for type-name-only labels. Migrated the value-hiding storage of `AuthorizationState`, `AuthorizationCode`, `AccessCredential`, `CredentialMaterial`, `MembershipId`, `CharacterId`, `ItemInstanceId`, `DefinitionVersion`, `PlanId`, and `ReadApiKey` onto `Redacted<T>` and their `Debug` onto `redact::opaque`, and replaced the hand-rolled `"<redacted>"` literals and `Option::map` markers in `token.rs`, `profile_gateway.rs`, `transfer.rs`, `config.rs`, and `ports.rs` with the shared constant and helpers. Every prior sanitized output is preserved exactly (including the type-name labels asserted by existing tests). Added `CorrelationId`: a bounded visible-ASCII, non-secret request identifier whose `Display` and `Debug` intentionally render the value, with rejection of empty/oversized/whitespace/control/non-ASCII values as a sanitized `ConfigurationError`. Full correlation propagation into requests, services, and any future diagnostic sink is deferred to a later task: carrying it through `TransferRequest`, the read/mutation services, and a diagnostic sink would cross unlisted surfaces and change many callers, so this task lands the reusable core contract only.
  - Checks: `cargo fmt --all -- --check` -> exit 0; `cargo test -p destiny-core` -> 69 passed, 0 failed; `cargo test -p destiny-adapters --all-features` -> 281 passed, 0 failed; `cargo test -p destiny-app` -> 31 passed, 0 failed; `cargo test -p destiny-app --features test-support` -> 202 passed, 0 failed; `cargo check --workspace --all-targets --all-features` -> exit 0 with no warnings; `git diff --check` -> exit 0. Focused redaction tests now live with the behavior: `redact.rs` unit tests for the wrapper/helper/opaque primitive, `ids.rs` tests for redacted opaque ids and non-secret printable `CorrelationId`, `auth.rs` tests for `AuthorizationState`/`AuthorizationCode`/`AccessCredential`, plus existing adapter `Debug` sentinel tests retained unchanged.
  - Runtime harness: N/A; unit/contract-level redaction behavior only.
  - Rollback: remove `crates/destiny-core/src/redact.rs`, revert the `Redacted<T>`/`redact::opaque`/`REDACTED` migrations in the listed files, and restore the prior per-type redaction and literals without changing business behavior.
  - Evidence: pending work-unit commit (parent owns the commit).

- [ ] **DHS-004 — Add bounded retry policy for transient read paths only**
  - Route: delegated multi-file writer because retry policy spans ports, app services, adapters, and tests.
  - Allowed edit surfaces: `crates/destiny-core/src/{ports.rs,error.rs}`, `crates/destiny-app/src/{sync.rs,queries.rs,manifest.rs,test_support.rs}`, `crates/destiny-adapters/src/{token.rs,manifest.rs,profile_gateway.rs,transfer.rs}`, and this tracker.
  - Outcome: pending.
  - Checks: retry contract tests for bounded transient/rate-limit attempts; mutation execution non-retry tests; `cargo test -p destiny-core -p destiny-app --features destiny-app/test-support`; focused adapter tests; `git diff --check`.
  - Runtime harness: N/A; deterministic retry fakes and scripted transports provide the boundary.
  - Rollback: remove retry policy additions and restore one-shot read transports plus existing status classification.
  - Evidence: pending.

- [ ] **DHS-005 — Prove cross-boundary leak absence across SQLite, CLI, and MCP**
  - Route: delegated multi-file writer because tests cross adapters, CLI, MCP, and shared test support.
  - Allowed edit surfaces: `crates/destiny-adapters/tests/hardening_leak_journey.rs`, `crates/destiny-adapters/tests/query_journey.rs`, `crates/destiny-mcp/tests/stdio.rs`, `crates/destiny-mcp/src/bin/offline_fixture.rs`, `crates/destiny-app/src/test_support.rs`, and this tracker.
  - Outcome: pending.
  - Checks: `cargo test -p destiny-adapters --test hardening_leak_journey`; `cargo test -p destiny-mcp --features offline-fixtures --test stdio`; relevant CLI runner focused tests; `git diff --check`.
  - Runtime harness: scan SQLite database bytes/sidecars, CLI stdout/stderr, MCP stdout/stderr, and serialized error shapes for shared secret sentinels with a positive control.
  - Rollback: remove the consolidated hardening leak journey and any fixture-only additions.
  - Evidence: pending.

- [ ] **DHS-006 — Lock fail-closed credential fallback policy and close WU-013**
  - Route: delegated verification and small doc/update task as needed.
  - Allowed edit surfaces: `crates/destiny-adapters/src/credential_store.rs`, `crates/destiny-mcp/src/main.rs`, `docs/integration-assumptions.md`, `docs/mvp-implementation-plan.md`, and this tracker.
  - Outcome: pending.
  - Checks: credential-store fail-closed tests; applicable MCP fixture tests; full baseline checks; independent full-range verification against WU-013 and AC-1/2/4/8/9.
  - Runtime harness: fail-closed missing/unsupported credential store tests; no plaintext/filesystem fallback selected.
  - Rollback: revert only fail-closed documentation/test additions, preserving prior native store behavior.
  - Evidence: pending.

## Acceptance criteria

- Completed WU-008 CLI read surface is present on the WU-013 branch before leak checks claim CLI coverage.
- Secret-bearing values redact consistently in `Debug` and `Display`; tests include tokens, codes, API keys, headers, URLs, payloads, membership IDs, and user-identifying sentinels where applicable.
- Transient/rate-limit retry behavior is bounded and applies only where safe; mutation execution steps never gain blind retries.
- Plan/step evidence remains visible for mutation execution, and any added correlation identity is safe, non-secret, and carried through typed diagnostics without exposing provider payloads.
- SQLite bytes/sidecars, CLI streams, MCP streams, serialized errors, and any diagnostics introduced by WU-013 do not contain secret sentinels.
- Credential storage remains fail-closed with no plaintext/filesystem/environment fallback in the MVP.
- Every completed task records observed checks, rollback boundary, and work-unit commit identity.

## Progress

- WU-012 is locally closed at `b2ad2a9` on `feature/destiny-mcp-mutation-tool`.
- WU-013 branch `feature/destiny-hardening` was created from `b2ad2a9`.
- Read-only mapping found WU-008 CLI code absent from `b2ad2a9` and present at `ae09804` on `feature/destiny-cli-read-journey`.
- User selected integrating WU-008 first and fail-closed-only credential fallback policy.
- `git merge-tree` shows expected integration conflicts around `README.md`, `Cargo.lock`, and adapter library/doc surfaces; dependency integration is a real first work unit.

## Verification evidence

- Read-only scout inspected `crates/**`, `docs/**`, `odd/tasks/**`, workspace manifests, and returned surface/gap mapping for redaction, logging, retry, plan/step IDs, credential fallback, and leak-test fixtures.
- DHS-002 integrated WU-008 CLI reads in commit `b34a729`; independent verification passed with no blocker/high/medium findings.
- DHS-003 consolidated redaction under `crates/destiny-core/src/redact.rs` and added the non-secret `CorrelationId` core contract; core/app/adapter focused tests and the workspace check pass. Risk noted: pi-lens reported stale rust-analyzer false positives (`!bool`, array-to-slice coercion) on lines byte-identical to `HEAD` in `crates/destiny-adapters/src/{transfer.rs,manifest.rs}` and `crates/destiny-app/src/config.rs`; rustc, `cargo check --workspace --all-targets --all-features`, and the test suites contradict them, matching the DHS-002 stale-LSP precedent.

## Closure

Open.

## Next step

Run DHS-004 as a delegated multi-file hardening task for the bounded retry policy on transient read paths only, and carry `CorrelationId` through typed diagnostics when a diagnostics sink is introduced.
