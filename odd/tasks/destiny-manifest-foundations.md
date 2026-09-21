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

- [x] **DMF-002 — Add publishable definition repository contracts and fake**
  - Route: delegated multi-file writer plus independent high-risk fallback verification; writer trigger fired because the slice touched multiple non-trivial files. Native assessment was schema-incompatible/unassessable under clone-disabled RDD, so the ordinary high-risk plan required a separate verifier.
  - Outcome: extended `DefinitionRepository` with atomic complete-set publication and strengthened the test-support in-memory fake with complete replacement, equal-set idempotence, duplicate/empty/mixed-version rejection before mutation, deterministic reads, attempt/count observability, and injected sanitized publication failure that preserves prior state.
  - Check: writer and independent verifier both passed fmt, 15 core tests, 38 app test-support tests, focused clippy `-D warnings`, and diff checks; LSP diagnostics were clean; parent readback and `git diff --check` passed. No blocking, major, or minor finding remained.
  - Evidence: commit `1827787` (`feat: add Manifest definition repository publishing`).
  - Rollback: revert only `crates/destiny-core/src/ports.rs` and the DMF-002 publication behavior/tests in `crates/destiny-app/src/test_support.rs`.

- [x] **DMF-003 — Implement Manifest validation and normalization service**
  - Route: delegated multi-file writer, independent high-risk fallback verification, one bounded correction, and repeated final verification; writer trigger fired because the slice touched multiple non-trivial files. Native assessment remained unavailable, so the separate verifier was required.
  - Outcome: added provider-neutral untrusted acquisition candidates and an application `ManifestService` that validates exact versions, bounded complete sets, duplicate hashes, text/tags, and type-level definition identity before atomically publishing deterministic normalized definitions. Provider casing is preserved for display/classification values; opaque versions remain exact; only search tags lowercase. Output is hash-ordered, tags are sorted/deduplicated, transformed lengths are rechecked, and publication returns a minimal summary without cloning the catalog. Independent review removed an invented static-definition flag and corrected display/version lowercasing before acceptance.
  - Check: final independent verification passed fmt, 15 core tests, 52 app test-support tests (14 Manifest tests), focused clippy `-D warnings`, diff/status checks, and commit-readiness review. Parent reran the focused 14-test Manifest harness and `git diff --check`; LSP diagnostics were clean. The 100,000-candidate safety ceiling has an exact boundary test and remains subject to real fixture validation in DMF-004.
  - Evidence: commit `9001ce1` (`feat: add Manifest validation service`). The 784-line source slice exceeds the ~400 heuristic because the cohesive service and its 14 behavior/security tests cannot be separated without an unusable intermediate contract; it remains an independent work-unit commit and no tests/docs were compressed.
  - Rollback: remove `crates/destiny-app/src/manifest.rs` and revert only the DMF-003 acquisition-contract/module/fake updates in the other four touched files.

- [ ] **DMF-004 — Implement bounded Bungie Manifest acquisition and mapping** *(in progress)*
  - Route: delegated multi-file writer after dependency/API revalidation; adapter and fixture surfaces; writer trigger fired because the slice touches multiple non-trivial files.
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
- DMF-002 added atomic, validated definition-set publication and the deterministic in-memory fake in `1827787`; independent verification passed all authorized checks with no blocking finding.
- DMF-003 added and corrected the provider-neutral validation/normalization/publication service in `9001ce1`; final independent verification passed. The recorded feature-chain slices are DMF-001 (`924ce89`), DMF-002 (`1827787`), and DMF-003 (`9001ce1`), each with its adjacent evidence commit where present.

## Verification evidence

Pending implementation.

## Next step

Delegate DMF-004 within the exact adapter/fixture edit surfaces and validate the real-catalog size assumption against the available offline fixture evidence.
