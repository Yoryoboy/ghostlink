# Destiny Manifest Foundations

## Objective

Implement WU-004 from `docs/mvp-implementation-plan.md`: acquire a bounded Bungie Manifest definition set, validate its version and completeness, normalize provider data into core records, and publish it to an offline in-memory definition repository without introducing SQLite.

## Problem

The workspace already owns normalized `ItemDefinition` records plus read-only Manifest/repository ports, but it cannot acquire, validate, normalize, or publish definitions. Later search, inspection, SQLite projection, and profile synchronization need a provider-neutral, deterministic definition boundary first.

## Why

Manifest data must remain a rebuildable provider projection rather than an application contract. A bounded, versioned, offline-testable pipeline prevents raw Bungie payloads, transport details, and future SQLite concerns from leaking into core or presentation layers.

## Scope

- Revalidate Bungie's current official Manifest index and per-component JSON contracts.
- Acquire the Manifest index and the English `DestinyInventoryItemDefinition` component through an adapter-owned transport seam.
- Validate the success envelope, non-empty version, exact required component path, bounded response sizes, and complete normalized records.
- Normalize only the definition/search fields required by MVP search and inspect behavior.
- Publish complete normalized sets atomically to a fake/in-memory `DefinitionRepository` for offline application tests.
- Preserve deterministic ordering and idempotent normalized output.

## Constraints

- No SQLite schema, migrations, durable persistence, profile state, inventory synchronization, live account, or broad definition catalog.
- No raw provider payloads, upstream messages, API keys, or transport details cross into core, application results, CLI/MCP output, errors, logs, fixtures, or model-facing values.
- `destiny-core` remains provider-, transport-, and serialization-free; adapters depend inward through core ports.
- Automated checks remain offline and credential-free. Live Manifest acquisition is a separate manual gate.
- Technical artifacts are written in English.

## Official evidence and decisions

Revalidated on 2026-09-21:

- Bungie's official `Destiny2.GetDestinyManifest` contract exposes `GET /Destiny2/Manifest/` and returns the current Manifest contract: <https://bungie-net.github.io/multi/operation_get_Destiny2-GetDestinyManifest.html>.
- The official `Destiny.Config.DestinyManifest` schema defines `version`, `jsonWorldContentPaths`, and locale-indexed `jsonWorldComponentContentPaths`; the component paths are explicitly warned as subject to change: <https://bungie-net.github.io/multi/schema_Destiny-Config-DestinyManifest.html>.
- Bungie's official API wiki documents that callers retrieve the Manifest index, choose a language/format path, then GET that file; JSON definitions are dictionaries keyed by string-form hashes: <https://github.com/Bungie-net/api/wiki/Obtaining-Destiny-Definitions-%22The-Manifest%22>.
- The official inventory item schema identifies the normalized MVP source fields, including `hash`, `displayProperties`, `itemTypeDisplayName`, `itemCategoryHashes`, `damageTypeHashes`, and the optional inventory block: <https://bungie-net.github.io/multi/schema_Destiny-Definitions-DestinyInventoryItemDefinition.html>.
- The inventory block's `bucketTypeHash`, `tierTypeName`, and `isInstanceItem` distinguish definition metadata from runtime item instances: <https://bungie-net.github.io/multi/schema_Destiny-Definitions-DestinyItemInventoryBlockDefinition.html>.

Decision: WU-004 uses the bounded English `DestinyInventoryItemDefinition` path from `jsonWorldComponentContentPaths`, not the SQLite/mobile database and not the aggregated all-definition JSON. Because Bungie marks component paths as unstable, the adapter validates their presence on every acquisition and maps absence/change to a sanitized provider-validation failure. The Manifest `version` is the `DefinitionVersion`; normalized equality is deterministic, while durable revision allocation remains WU-005 scope.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 700–1,100 authored changed lines across contracts, services, adapter transport/mapping, fixtures, tests, and documentation.
- Chain strategy: `feature-branch-chain`.
- Branch: `feature/destiny-manifest-foundations`, chained from completed WU-003 at `84a7a16`.
- Engram mirror topic: `odd/destiny-manifest-foundations/tasks`.
- Native RDD: disabled for this clone by prior explicit user decision; ordinary risk assessment and delegated verification apply.

## Effective checks

- TDD mode: not configured; use ordinary functional checks without claiming strict RED/GREEN evidence.
- Baseline: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --locked`; `cargo test --workspace --all-features --locked`; CLI/MCP help.
- Focused: core contract tests; `cargo test -p destiny-app --features test-support --locked`; `cargo test -p destiny-adapters --all-features --locked`; fixture acquisition, malformed/incomplete input, idempotence, ordering, repository replacement, response-bound, and raw-payload sentinel tests.
- Runtime harness: deterministic fixture acquisition through the application service into the in-memory repository. Live Bungie acquisition is N/A for automated closure because it requires an API key and network.

## Tasks

- [x] **DMF-001 — Establish evidence and feature boundary**
  - Route: delegated repository mapping plus parent primary-source research and tracking; mapping trigger fired because understanding spans more than four files.
  - Outcome: created the chained branch and durable ODD tracker; recorded official acquisition/schema evidence, bounded MVP definition scope, exclusions, task slices, forecast, and checks.
  - Check: repository tracker and Engram mirror readback passed; `git diff --check` passed; branch was clean after the work-unit commit.
  - Evidence: commit `924ce89` (`chore: start Destiny Manifest foundations`).
  - Rollback: remove only this feature branch and tracker before implementation begins.

- [ ] **DMF-002 — Add publishable definition repository contracts and fake** *(in progress)*
  - Route: delegated multi-file writer; core/app/test-support surfaces; writer trigger fired because the slice touches multiple non-trivial files.
  - Outcome: extend the provider-neutral repository contract for atomic complete-set publication and implement a deterministic in-memory fake with lookup, version, replacement, and idempotence contract coverage.
  - Check: focused core and app test-support commands plus formatter/clippy and writer readback.
  - Rollback: remove only the new publication contract/fake behavior and associated tests.

- [ ] **DMF-003 — Implement Manifest validation and normalization service**
  - Route: delegated multi-file writer; app/core contract surfaces.
  - Outcome: validate acquired versioned input, normalize bounded inventory definition/search fields, reject malformed/incomplete/duplicate data, distinguish static definitions from runtime instances, produce deterministic ordering, and publish only complete sets.
  - Check: focused service tests for malformed/incomplete inputs, definition/instance distinction, deterministic/idempotent output, atomic failure, and raw-payload isolation.
  - Rollback: remove only the application Manifest service and its tests while preserving repository contracts.

- [ ] **DMF-004 — Implement bounded Bungie Manifest acquisition and mapping**
  - Route: delegated multi-file writer after dependency/API revalidation; adapter and fixture surfaces.
  - Outcome: fetch and validate the Manifest index plus required English component through a bounded transport seam, map sanitized provider data to the application input contract, and prove the full offline fixture pipeline.
  - Check: focused adapter tests for request shape, success envelope, version/path selection, redirects/proxy/TLS/deadline/size/content type, malformed JSON, provider errors, mapping completeness, and sentinel redaction; full fixture harness into the fake repository.
  - Rollback: remove only the Manifest adapter/fixtures and declarations without touching SQLite or OAuth.

- [ ] **DMF-005 — Verify and close WU-004**
  - Route: native read-only assessment followed by the required ordinary verification plan; delegate broad command execution.
  - Outcome: independently verify the complete WU-004 range, record honest automated/manual gates, and close the feature.
  - Check: all baseline and focused commands, dependency/diff/security review, deterministic fixture harness, and final clean status.
  - Rollback: reset this feature branch to `84a7a16` to remove only WU-004 changes.

## Acceptance criteria

- The application can acquire a versioned Manifest definition set through a port and publish it atomically to an in-memory repository.
- Missing, malformed, duplicate, inconsistent-version, incomplete, oversized, or provider-error inputs fail closed with sanitized typed errors and preserve the previous repository state.
- Normalized definitions are deterministic, idempotent, bounded to MVP search/inspect needs, and distinguish definition hashes from runtime instance identifiers.
- Search-oriented fields are sanitized and stable; raw provider payloads never become core, application, CLI, or MCP contracts.
- Offline contract and integration tests cover the gateway, service, and repository fake without credentials or network.
- SQLite and profile/inventory synchronization remain untouched.

## Progress

- WU-003 is complete at `84a7a16` on `feature/destiny-oauth-foundations`.
- WU-004 branch created from that exact completed boundary.
- Repository mapping identified existing `ManifestGateway`, `ManifestDefinitions`, read-only `DefinitionRepository`, normalized `ItemDefinition`, `InMemoryDefinitionRepository`, and `QueuedManifestGateway` seams.
- Official evidence resolves the acquisition source: use the English per-component JSON path for `DestinyInventoryItemDefinition`, validate it on every index response, and avoid SQLite/all-definition payloads.
- DMF-001 established and committed the feature boundary in `924ce89`.

## Verification evidence

Pending implementation.

## Next step

Delegate DMF-002 within the exact core/app/test-support edit surfaces.
