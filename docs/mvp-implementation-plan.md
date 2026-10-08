# Destiny MVP Implementation Plan

This plan turns the verified MVP architecture into dependency-correct, independently reviewable work units. It preserves runnable vertical progress: every adapter is thin, every provider detail enters through a port, and each slice leaves the repository in a useful state. The plan is implementation guidance, not scaffolding; no source code is added by DMA-004.

## Current scope update: single-item move and equip

`ghostlink equip <ITEM> --character <CHARACTER> [--json]` and default-enabled MCP
`equip_item` now share `plan_equip` / `execute_equip_verified`. Both require explicit
selectors, refuse ambiguity and unsafe class/exotic/equipped-elsewhere cases,
transfer automatically if needed, force fresh preflight, refresh after attempted
actions, and verify target location **and equipped state**. One call authorizes
intent; there is no separate approval artifact.

`ghostlink move <ITEM> (--character <id|class> | --vault) [--json]` now exposes
MCP `move_item` semantics through the same `plan_move` / `execute_move_verified`
pipeline, without a CLI dependency on MCP. Exactly one explicit destination is
required; IDs take precedence and ambiguous selectors are refused. Already at
the destination during planning is a `SameLocation` refusal (CLI exit 40), not
a no-op. Reaching it between planning and forced preflight can be a verified
no-op (0); other changed preflight locations refuse before dispatch. Provider
restrictions such as equipped items can reject after dispatch. There are no
retries or rollback; refresh and destination verification follow attempted steps.
The move text/JSON contract is transport-neutral in `destiny-output`, separate
from equip; exit policy remains CLI-owned. Offline tests do not prove live success.

`destiny-output` is the neutral shared equip JSON renderer; CLI text and exit
codes remain CLI-owned. Production MCP composes the exact EquipItem-only adapter
on the same credential session and serialized mutation seam as transfers.
Orbit/social/offline restrictions are enforced by Bungie, not an activity
pre-check. Partial progress has no rollback; uncertain outcomes must not be
blindly retried. Offline parity and child-process fixtures are evidence of the
pipeline, not live-provider success; authorized live smoke remains separate.

Build planning/apply, multi-item execution, and plug/mod/perk mutations remain
deferred. The original work-unit exclusions below describe those earlier slices,
not a reversal of this current scope update.

## Planning assumptions and principles

- Bungie remains the remote source of truth; SQLite is a rebuildable cache, projection, and search index.
- The core owns domain types, policy, application services, and behavioral ports. Adapters own transport, serialization, storage mechanisms, and presentation.
- Core ports and adapters are strictly separated. CLI and MCP never call Bungie or SQLite directly, and provider payloads never become application contracts.
- Early units use fakes and fixtures. No live Bungie dependency is introduced before the contracts, state machine, persistence, and vertical read path are testable offline.
- The public OAuth path is explicit: one exact configured case-sensitive callback listener, system browser, no client secret, no refresh token, and explicit reauthorization after expiry or rejected access. No PKCE, wildcard loopback, dynamic port, custom scheme, or device flow is assumed without official-source revalidation.
- Provider-specific endpoint/component choices, transfer sequencing, rate-limit fields, credential-store libraries, and `rmcp` APIs require implementation-time revalidation against official/current sources. The plan records decisions, not permanent provider guarantees.
- For MCP, explicit user intent to move an item is sufficient authorization for one direct `move_item` call. Semantic resolution, ambiguity rejection, mandatory fresh preflight, current-state validation, minimum remote calls, partial-failure reporting, post-action refresh, and verification remain mandatory. There is no mandatory preview, confirmation token/value, approval ceremony, or two-step MCP protocol. CLI may retain normal interactive UX without defining MCP authority.
- Approximately 400 authored changed lines is a review heuristic, not a code or documentation cap. A unit that naturally exceeds it becomes a chained review slice rather than losing tests or clarity.

## Proposed workspace and ownership

Use one Cargo workspace without needless microcrates. Start with a small number of cohesive crates; split only when ownership or dependency direction requires it.

```text
crates/
  destiny-core/       domain IDs, records, errors, ports, policies, test doubles
  destiny-app/        authorization, sync, query, planning, execution services
  destiny-adapters/   Bungie HTTP/OAuth, Manifest, SQLite, credential store, clock/IDs
  destiny-output/     transport-neutral equip JSON and move/lock text/JSON projections
  destiny-cli/        command parsing, terminal UX, process exit policy
  destiny-mcp/        MCP stdio server, tool schemas, sanitized result mapping
```

`destiny-core` must remain provider- and transport-free. `destiny-app` depends inward on core ports. `destiny-adapters` implements those ports and may be subdivided into modules (`bungie`, `manifest`, `sqlite`, `credentials`) rather than crates. CLI and MCP depend on application services only. Tests may use fixture and fake modules owned by core/app; live integration tests remain explicit and isolated.

The first scaffold must establish these later commands, with names kept stable unless a documented repository decision changes them:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo run -p destiny-cli -- --help
cargo run -p destiny-mcp -- --help
```

The scaffold must also document the supported feature/configuration matrix and how offline fixture tests are selected. `cargo test --workspace --all-features` remains credential-free and offline: live integration tests are ignored or excluded from baseline commands and require a separate explicit opt-in command/configuration, defined when those tests are implemented. Later units must not invent unrelated runner commands; they extend these commands with focused package/test filters only when the package exists.

## Ordered work units

### WU-001 — Workspace, configuration, and error foundations

**Purpose/outcome:** Create the workspace, package boundaries, shared configuration parsing/validation, sanitized diagnostic setup, stable adapter-neutral error taxonomy, and the baseline commands above. The empty application starts and fails clearly on missing required configuration.

**Dependencies:** None.

**Likely ownership/edit surfaces:** `Cargo.toml`, crate manifests; `crates/*/src/lib.rs` and `main.rs`; `config` and `error` modules; workspace README/config example; CI or local command documentation if present.

**Behavior and checks:** Unit-test configuration parsing, exact callback URI validation, required public client/API key handling, and error classification. Integration-check that each binary starts/help-renders without credentials. Contract-check that presentation layers can only receive sanitized typed errors. Run the scaffold commands, with unit/integration checks and a manual review of secret redaction.

**Exclusions:** No OAuth exchange, live HTTP, SQLite schema, endpoint calls, or real credential persistence.

**Review notes:** Keep dependency direction visible in manifests. Revalidate Rust and selected library versions before pinning; do not add a confidential-client path. Internal review slices: (a) workspace/manifests and dependency graph, then (b) configuration/errors and baseline command checks. Roll back either slice by removing only its manifests/modules and associated tests.

### WU-002 — Domain records, ports, policies, and test doubles

**Purpose/outcome:** Establish opaque IDs and domain records, freshness/revision semantics, typed outcomes, behavioral ports, mutation policy seams, and deterministic clocks/ID generators. Provide in-memory/fake gateways sufficient to exercise application behavior offline.

**Dependencies:** WU-001.

**Likely ownership/edit surfaces:** `destiny-core` domain/ports/error modules; `destiny-app` test doubles and fixtures.

**Behavior and checks:** Unit-test ID separation (definition hash versus instance ID), locations, plan expiry, revision binding, freshness states, ambiguity, typed failures, and semantic mutation-policy inputs. Contract-test fake ports against the expected application boundary. Snapshot-test sanitized result shapes where stable. No live or database checks yet.

**Exclusions:** No Bungie JSON mapping, SQL, CLI/MCP schemas, or provider-specific endpoint assumptions.

**Review notes:** This is the main anti-coupling seam. Keep test doubles behaviorally useful, not provider-shaped. Internal review slices: (a) IDs/records/errors, then (b) ports/policies/fakes. Each slice has its own tests and can be rolled back by removing only the corresponding core/app modules and fixtures.

### WU-003 — Credential store and public OAuth callback state machine

**Purpose/outcome:** Implement secure credential-store port wiring and the public OAuth flow: browser launch boundary, exact configured callback listener, one-time random state, code exchange without client secret, and explicit reauthorization outcomes.

**Dependencies:** WU-001–002.

**Likely ownership/edit surfaces:** `destiny-app` authorization service; `destiny-adapters::credentials` and `::bungie::oauth`; callback listener module; platform configuration docs.

**Behavior and checks:** Unit-test pending-state expiry, replay, cancellation, denial, mismatch, exchange failure, 401/expiry to `reauthorization_required`, logout, and absence of refresh-token behavior. Contract-test fake browser, callback, token exchange, and credential store. Integration-test a restricted fake credential backend. Manual-check that secret-bearing values never appear in errors, logs, SQLite, MCP values, or CLI output. Live OAuth is a separate checklist item, not a prerequisite for offline green checks.

**Exclusions:** No confidential client, refresh flow, dynamic/wildcard callback, PKCE, device flow, or plaintext fallback. An unavailable OS store fails closed unless an explicitly gated, documented restricted fallback is approved.

**Review notes:** Official Bungie behavior and exact callback provisioning must be revalidated at implementation time. The callback is a deployment contract, not a convenience default.

### WU-004 — Manifest acquisition, validation, normalization, and fake repository

**Purpose/outcome:** Download or receive Manifest data through a port, validate version/completeness, normalize required definitions, and publish them to a fake/in-memory definition repository for offline application tests. This unit does not establish SQLite.

**Dependencies:** WU-001–002.

**Likely ownership/edit surfaces:** `destiny-app::manifest`; `destiny-core` definition repository contract; `destiny-adapters::manifest` acquisition/mapping module; fake/in-memory repository fixtures.

**Behavior and checks:** Unit-test version selection, malformed/incomplete input, definition/instance distinction, and idempotent normalized output. Integration-test fixture acquisition into the fake/in-memory definition repository. Contract-test the Manifest gateway and repository fakes. Snapshot-test normalized definition records and sanitized search fields. Manual check that raw provider payloads are not surfaced. SQLite schema, migrations, transactional replacement, and persistence checks belong to WU-005.

**Exclusions:** No profile state, inventory synchronization, live-account requirement, or broad definition catalog beyond MVP search/inspect needs.

**Review notes:** Provider download URLs and completeness signals are implementation-time official-source questions. Rollback boundary: remove Manifest acquisition/normalization and fake repository modules without touching SQLite.

### WU-005 — SQLite schema, migrations, projections, freshness, and revisions

**Purpose/outcome:** Establish SQLite schema/migrations and transactional persistence for Manifest definitions and profile projections, plus durable non-secret repositories for characters, item projections, sync metadata, revisions, plans/execution records, and migration/version management.

**Dependencies:** WU-002 and WU-004 normalized definition/repository contracts.

**Likely ownership/edit surfaces:** `destiny-adapters::sqlite`; migration files; repository interfaces and app mapping tests.

**Behavior and checks:** Unit-test freshness policy, changed/unchanged revision allocation, dataset-specific TTL decisions, plan binding, and sanitized serialization. Integration-test migrations, transactional snapshot replacement, concurrent/single-flight behavior, rollback preservation, and no secret columns/values. Contract-test repository implementations against ports. Snapshot-test stable projection rows/results.

**Exclusions:** No raw API payload archive, tokens, API keys, authorization codes, or provider HTTP logic.

**Review notes:** SQLite is a projection, never an authority. Schema changes must remain reversible or have an explicit migration path. Internal review slices: (a) schema/migrations and definition persistence, then (b) profile/projection repositories, revisions, and transactional replacement. Each slice has a migration rollback boundary; remove only the slice's migrations/repository modules while preserving the WU-004 fake repository.

### WU-006 — Bungie profile normalization and offline synchronization

**Purpose/outcome:** Map verified profile components into normalized characters/items, apply location and item semantics, and atomically replace the affected projection with freshness/source/revision metadata.

**Dependencies:** WU-003–005.

**Likely ownership/edit surfaces:** `destiny-app::sync`; `destiny-adapters::bungie::profile`; normalizers and fixture payloads.

**Behavior and checks:** Unit-test normalization for characters, Vault, equipped items, unknown locations, locks, quantities, perks summaries, and malformed/partial components. Contract-test Bungie gateway requests and normalized outcomes using fakes. Integration-test fixture sync, forced/stale/preflight/post-action reasons, idempotency, changed revisions, and safe partial provider errors. Snapshot-test normalized inventory. Manual/live check is deferred to the live integration checklist.

**Exclusions:** No mutation calls, build engine, duplicate ranking, equip, Postmaster, or world-state data.

**Review notes:** Exact profile components and response fields require official-source revalidation. Never claim remote atomicity.

### WU-007 — Search and inspect query services

**Purpose/outcome:** Implement authorization/freshness-aware `list_characters`, semantic `search_inventory`, and `inspect_item` over projections and Manifest definitions, with ambiguity and stale-state behavior.

**Dependencies:** WU-004–006.

**Likely ownership/edit surfaces:** `destiny-app::queries`; `destiny-core` query contracts; SQLite search indexes/projections; fixture corpus.

**Behavior and checks:** Unit-test name/type/perk filtering, instance selection, ambiguity, missing item, freshness, and definition-versus-instance reporting. Integration-test queries against fixture snapshots and stale-triggered synchronization. Contract-test equivalent sanitized application results. Snapshot-test CLI-neutral query results. No live check required beyond the separately listed checklist.

**Exclusions:** No CLI formatting, MCP transport, ranking/optimization, build matching, or mutation.

**Review notes:** Keep semantic search stable while indexes evolve; do not expose unbounded provider records.

### WU-008 — CLI read journey

**Purpose/outcome:** Deliver a usable read-only CLI path through shared services: authorization handling, sync-on-policy, `list_characters`, inventory search, inspect, bounded JSON, and typed exit behavior.

**Dependencies:** WU-003–007.

**Likely ownership/edit surfaces:** `destiny-cli` command/parser/output/confirmation modules; CLI fixtures and integration tests.

**Behavior and checks:** Unit-test argument validation and sanitized rendering. Integration-test the offline fixture journey for list/search/inspect, stale refresh, ambiguity, and typed failures. Snapshot-test human and JSON output. Contract-test that CLI uses application services rather than adapter internals. Manual-check system-browser and reauthorization UX boundaries.

**Exclusions:** No CLI-specific business logic, mutation execution, raw Bungie command, or live-only success claim.

**Review notes:** This is the first user-visible runnable vertical slice; keep output concise and reviewable.

### WU-009 — Mutation planning, intent authorization, and policy

**Purpose/outcome:** Add internal `move_item` planning and shared mutation policy. Plans contain source, destination, Vault intermediary when required, expected effects, bounded expiry, selected revision, and current-state assumptions. MCP explicit user intent authorizes the single direct tool call; CLI may add normal interactive confirmation UX.

**Dependencies:** WU-006–007.

**Likely ownership/edit surfaces:** `destiny-app::mutation::plan` and policy; core plan/mutation types; CLI-neutral result contracts.

**Behavior and checks:** Unit-test direct moves, Vault-mediated moves, unsupported locations, ambiguity rejection, expiry, changed revision, wrong destination/item, and explicit user intent versus absent or ambiguous intent. Contract-test semantic inputs and sanitized effect/result shapes. Integration-test that planning has no remote side effect, MCP has no preview/token/two-step requirement, and the shared policy enforces fresh preflight inputs. Snapshot-test action effects. Revalidate official/current lock, availability, and transfer constraints during implementation planning before finalizing provider assumptions.

**Exclusions:** No Bungie mutation call, no adapter-forged authority, no multi-item plans, equip, dismantle, or rollback claim.

**Review notes:** The internal plan is a safety and state-binding object, not an MCP approval artifact. Rollback boundary: remove mutation planning/policy modules and their tests without touching the transfer adapter.

### WU-010 — Bungie transfer execution with preflight, partial failure, refresh, and verification

**Purpose/outcome:** Implement the shared executor and Bungie adapter boundary for one-item transfer: mandatory fresh preflight, re-resolution, minimum supported calls, step recording, honest partial failure, post-action refresh, and destination verification.

**Dependencies:** WU-003, WU-006, WU-009.

**Likely ownership/edit surfaces:** `destiny-app::mutation::execute`; `destiny-adapters::bungie::transfer`; execution records and fixture gateway.

**Behavior and checks:** Unit-test preflight invalidation, expired plan, already-satisfied postcondition, step ordering, transient/non-transient errors, partial failure, retry refusal without refresh, and verification failure. Contract-test Bungie gateway request/response mapping with a fake. Integration-test the complete transfer against a deterministic fake, including post-refresh and changed state. Snapshot-test semantic outcomes and bounded step results. Manual/live test only with a non-production account after official endpoint/component revalidation.

**Exclusions:** No claim of multi-call atomicity or automatic rollback; no broad action framework, equip, locking, or multi-item execution.

**Review notes:** This is the highest-risk unit. Keep remote calls behind one narrow gateway and preserve evidence of completed/uncompleted steps. Revalidate official/current lock, availability, and transfer constraints immediately before implementation and record source/version evidence. Internal review slices: (a) preflight and provider-call sequencing, then (b) partial-failure recording, post-action refresh, and verification. Each slice is independently revertible at the executor/adapter boundary.

### WU-011 — MCP stdio read tools

**Purpose/outcome:** Add the thin stdio server and semantic read tools (`list_characters`, `search_inventory`, `inspect_item`) over the same application services and sanitized result types.

**Dependencies:** WU-007; WU-001 command baseline. MCP reads depend on shared query services/scaffold, not CLI completion (WU-008).

**Likely ownership/edit surfaces:** `destiny-mcp`; RMCP feature/configuration; tool schemas and transport integration tests.

**Behavior and checks:** Unit-test tool argument/result mapping and bounded errors. Contract-test MCP results against CLI-neutral application results. Integration-test stdio request/response with fixture services. Snapshot-test tool outputs. Manual-check no credentials, authorization codes, headers, or unbounded payloads cross the MCP boundary. Revalidate and pin official `rmcp` APIs/features before implementation.

**Exclusions:** No direct Bungie calls, mutation tools, HTTP MCP transport, or model-specific logic.

**Review notes:** Keep RMCP compatibility isolated so SDK churn cannot spread into core.

### WU-012 — MCP single-call intent-authorized mutation tool

**Purpose/outcome:** Expose one semantic `move_item` tool through MCP using the shared planner/executor. Explicit user intent to move an identified item to a destination is sufficient authorization; the service still performs semantic resolution, ambiguity rejection, fresh preflight, minimum remote calls, partial-failure reporting, refresh, and verification. No mandatory preview, confirmation token/value, approval ceremony, or two-step protocol exists.

**Dependencies:** WU-009–011.

**Likely ownership/edit surfaces:** `destiny-mcp` mutation tool modules; shared MCP/application contract tests; documentation of host behavior.

**Behavior and checks:** Unit-test ambiguous/missing semantic target, stale revision, repeated request handling, preflight changes, and sanitized partial failure. Contract-test parity with CLI services and exact single-call semantics. Integration-test one stdio `move_item` request with fakes, proving no approval artifact or second tool call is required while preflight/policy remain enforced. Snapshot-test effects and typed outcomes. Manual-check default-enabled MCP mutation and secret exclusion.

**Exclusions:** No preview tool, confirmation token/value, approval ceremony, two-step MCP protocol, adapter-generated authority, bypass of preflight/policy, or new mutation semantics.

**Review notes:** This unit is review-sensitive because explicit user intent is the MCP authorization decision while the host/model remains untrusted for credentials and semantic ambiguity. Rollback boundary: remove only MCP mutation schemas/mapping and their transport tests; shared mutation behavior remains intact.

### WU-013 — Security, logging, and secret-leak checks

**Purpose/outcome:** Harden all paths with redaction, bounded retries, a non-secret correlation identity (a core `CorrelationId` contract only; carrying it through requests, services, and typed diagnostics is deferred), visible plan/step identifiers, a fail-closed credential policy with no fallback, and automated leak checks across the SQLite stores and the CLI and MCP process streams.

**Dependencies:** WU-003, WU-005, WU-008, WU-010–012.

**Likely ownership/edit surfaces:** shared redaction and correlation contracts; credential-store policy tests; adapter read-retry paths; security test fixtures; configuration docs. No logging or diagnostics sink is introduced in this unit.

**Behavior and checks:** Unit-test redaction of tokens, codes, API keys, headers, URLs, payloads, and user-identifying values. Integration-test SQLite bytes/sidecars, CLI output, and MCP serialization/process streams with secret sentinels plus non-secret positive controls. Contract-test the bounded retry policy (only transient/rate-limit cases) and the fail-closed credential policy (missing, legacy, unavailable, and unsupported store outcomes, with no plaintext, filesystem, or environment fallback). Snapshot-test safe error shapes. Because the MVP has no logging framework, the pipeline tests the no-log/no-leak process-stream contract rather than an introduced logger. Manual security review and optional local secret scanner; report the exact tool/command only once established by the scaffold.

**Exclusions:** No hosted backend, threat-model expansion into unrelated platform security, no introduced logging or diagnostics sink, and no plaintext, filesystem, or environment credential fallback (the MVP has no fallback gate to opt into).

**Review notes:** Treat any leak as a release blocker; avoid logging “temporarily” for live debugging. Internal review slices: (a) redaction/retry behavior, then (b) cross-boundary leak tests and the fail-closed credential policy. Revert each slice independently with its diagnostics/tests. `CorrelationId` lands as a non-secret core contract only; carrying it through requests, services, and a future diagnostic sink is deferred, and plan/step identifiers already remain visible for mutation execution without it. The leak harness drives a real `destiny-cli` child only through the sanitized failure and rejected-request paths: the offline success path is unreachable because the production composition loads the OS-keyring credential before it opens the store, so success-path streams are proven through the CLI runner that emits the exact process bytes.

### WU-014 — End-to-end fixture journey

**Purpose/outcome:** Demonstrate the complete offline journey: authorization fake, Manifest fixture, profile sync, search/inspect, semantically resolve “The Last Word to Warlock,” authorize the MCP move by explicit user intent (with optional CLI confirmation UX), fake transfer, refresh, and verification.

**Dependencies:** WU-003–013.

**Likely ownership/edit surfaces:** shared fixture corpus; workspace integration/e2e test package; CLI/MCP journey harnesses; test documentation.

**Behavior and checks:** End-to-end integration test for the named journey, including stale cache, internal plan/state binding, fresh preflight, partial failure alternative, and successful verification. Contract-test equivalent CLI/MCP service outcomes, including one direct MCP call. Snapshot-test key journey outputs. Manual run through both adapters using the scaffold-established commands, without a live account.

**Exclusions:** No claim that fixtures prove Bungie compatibility, no live credentials, and no multi-call atomicity claim.

**Review notes:** Keep fixtures deterministic and sanitized so the test remains runnable in CI.

**Offline evidence (local WU-014 slices, 2026-09-24):** The real-adapter `transfer_execution_journey` composes the named "The Last Word to Warlock" path across the profile/transfer gateways, SQLite, shared query and mutation services, scripted transports, deterministic clock, fresh preflight, post-action refresh, and verified destination. Its adjacent scenarios cover stale preflight and honest partial failure. `query_journey` runs CLI JSON and MCP read-service projections against the same committed fixture state and pins deterministic key-output JSON (characters, Vault instance, definition, perks, metadata, privacy and missing result) without a new snapshot dependency. MCP `stdio` separately proves a single semantic `move_item` request over an in-memory fixture, including two Vault-mediated steps and verified destination; it does **not** use the real-adapter fixture. Run `cargo test -p destiny-adapters --all-features --test transfer_execution_journey`, `cargo test -p destiny-adapters --all-features --test query_journey`, and `cargo test -p destiny-mcp --all-features --test stdio` for focused evidence; the baseline remains `cargo test --workspace --all-features`. CLI mutation does not exist, and the CLI success child-process path still requires an OS credential store; parity is on shared read semantics, not byte-identical output or mutation. Fixtures do not prove live Bungie, real keyring, or multi-call atomicity. Detailed commits, test results and deferred checks: `odd/tasks/destiny-offline-fixture-journey.md`.

### WU-015 — Live integration checklist and packaging documentation

**Purpose/outcome:** Document and, where authorized, manually validate deployment prerequisites: exact registered callback, public client configuration, API key handling, Manifest/profile/transfer endpoint assumptions, non-production account, rate limits, RMCP pin, OS credential store, packaging paths, and recovery behavior.

**Dependencies:** WU-010–014 and implementation-time official-source revalidation.

**Likely ownership/edit surfaces:** `docs/` integration/deployment/runbook files; package metadata/examples; live-test checklist and release notes.

**Behavior and checks:** Manual/live checks only: OAuth callback exactness, denial/reauthorization, read sync, one transfer, post-refresh verification, and MCP stdio host compatibility. Record provider/version/date evidence and never commit credentials or live identifiers. Contract-check packaged CLI/MCP startup and configuration errors. No invented command is required beyond the scaffold commands and commands documented by the implementation.

**Exclusions:** No production release, hosted service, confidential backend, or guarantee that undocumented provider behavior remains stable.

**Review notes:** This is an operational gate and evidence package, not a substitute for automated tests.

## Feature flags and configuration gates

Keep gates narrow and safety-driven: `offline_fixtures`/test-only services, explicit `live_integration` configuration, and provider/API compatibility diagnostics. The credential store is fail-closed with no fallback gate: the MVP defines no plaintext, filesystem, or environment opt-in, so there is no credential-store fallback flag to add. `cargo test --workspace --all-features` remains credential-free/offline; live tests are ignored or excluded from it and run only through the explicit opt-in command/configuration defined when implemented. Do not gate MCP mutation out of the default MVP; its single-call intent-authorized contract is the feature. Do not add speculative feature flags for deferred World State, builds, or advanced actions.

## Review workload and slice forecast

The implementation should normally produce one work-unit commit per WU, with tests/docs alongside behavior and Conventional Commit messages. Group only tightly coupled units when a standalone landing state is impossible. Suggested review sequence:

1. Foundation: WU-001–003 (configuration, contracts, authorization).
2. Data plane: WU-004–006 (Manifest, SQLite, profile sync).
3. Read vertical: WU-007–008 (queries and CLI).
4. Mutation core: WU-009–010 (plan and execution).
5. MCP: WU-011–012 (reads and single-call intent-authorized mutation).
6. Hardening and evidence: WU-013–015.

Each slice should target roughly one reviewer session and monitor authored additions plus deletions near 400 lines. Broad WU-001, WU-002, WU-005, and WU-013 already declare internal review slices and rollback boundaries above; use those boundaries before creating chained PRs. If a slice exceeds the heuristic, use the listed WU/internal boundary; do not compress tests or documentation. Every slice states its start state, dependencies, finished behavior, exclusions, checks, and rollback boundary in its commit/PR description. No commit SHA is predicted here.

## Explicit non-goals and post-MVP boundary

The current scope includes single-item move/equip, lock/unlock (CLI and MCP `set_lock_state`), and weekly read capabilities. Lock uses shared fresh preflight, one dispatch, post-action refresh and lock-state verification; already satisfied is a verified no-op. Vault items use one owned character for the provider call (live validation pending). Build engines, multi-item build plans/apply, plug/mod/perk mutations, dismantling, Postmaster, duplicate ranking, web UI, hosted/confidential backend, general-purpose Bungie proxying, and broad inventory actions remain deferred. Later capabilities may reuse the ports and projections but must not bypass fresh preflight, bounded intent, or post-action verification.

## Architecture traceability

| Work unit | Architecture acceptance criteria covered |
|---|---|
| WU-001 | AC-1, AC-2, AC-8, AC-9 |
| WU-002 | AC-3, AC-4, AC-5, AC-6, AC-7 |
| WU-003 | AC-1, AC-2, AC-8, AC-9 |
| WU-004 | AC-3, AC-5, AC-9 |
| WU-005 | AC-3, AC-6, AC-8, AC-9 |
| WU-006 | AC-3, AC-5, AC-6, AC-9 |
| WU-007 | AC-3, AC-5, AC-6 |
| WU-008 | AC-4, AC-5, AC-8, AC-9 |
| WU-009 | AC-4, AC-6, AC-7 |
| WU-010 | AC-6, AC-9 |
| WU-011 | AC-4, AC-8, AC-9 |
| WU-012 | AC-4, AC-6, AC-7, AC-8, AC-9 |
| WU-013 | AC-1, AC-2, AC-4, AC-8, AC-9 |
| WU-014 | AC-3, AC-4, AC-5, AC-6, AC-7, AC-9 |
| WU-015 | AC-1, AC-3, AC-4, AC-6, AC-8, AC-9 |

Architecture criteria referenced above: **AC-1** public exact-callback authentication; **AC-2** typed authorization failure/reauthorization; **AC-3** fresh, revisioned, atomic Manifest/profile snapshots; **AC-4** equivalent CLI/MCP shared-service results and one-call intent-authorized MCP mutation; **AC-5** concrete instance-aware search/inspect; **AC-6** semantic resolution, ambiguity rejection, preflight, partial failure, refresh, verification; **AC-7** default-enabled intent-authorized MCP execution; **AC-8** no secret leakage; **AC-9** repeatable vertical fixture journey without atomicity claims.

## Recommended first unit and readiness

Start with **WU-001**. It is small enough to review, establishes the command contract for every later unit, and makes configuration/error failures observable before any provider coupling exists. The plan is implementation-ready for offline scaffolding. Remaining blockers are implementation-time official-source revalidation of Bungie endpoint/component details and callback provisioning, current `rmcp` compatibility, first-class OS credential stores/fallback policy, and the selected platform packaging matrix; none requires changing the dependency order or adding live Bungie work to the early units.
