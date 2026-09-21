# Destiny Workspace Foundations

## Objective

Implement WU-001 from `docs/mvp-implementation-plan.md`: establish the Rust workspace, dependency boundaries, configuration validation, sanitized diagnostics, adapter-neutral errors, and stable offline baseline commands.

## Problem

The repository has an approved architecture and implementation sequence but no Rust workspace or executable foundation. Later OAuth, persistence, CLI, and MCP work needs stable crate boundaries and safe startup failures first.

## Why

A small provider-free foundation makes dependency direction, configuration errors, and secret redaction testable before any external integration is introduced.

## Scope

- Create the five-crate Cargo workspace defined by the implementation plan.
- Establish inward dependency direction without provider-specific behavior.
- Implement shared configuration parsing and exact callback URI validation.
- Implement an adapter-neutral, sanitized error taxonomy and diagnostic setup.
- Make both binaries render help without credentials and fail clearly when required runtime configuration is absent.
- Document configuration and baseline offline commands.

## Constraints

- No OAuth exchange, live HTTP, SQLite, provider endpoints, or credential persistence.
- Public-client configuration only; no client secret or confidential-client path.
- Tests and baseline all-features checks must be credential-free and offline.
- Technical artifacts are written in English.
- This is Organic Driven Development, not SDD/OpenSpec.
- Keep the workspace cohesive; do not introduce additional crates without a demonstrated ownership boundary.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 350–500 authored changed lines across manifests, scaffold modules, tests, and documentation.
- Chain strategy: pending only if the running authored-line count exceeds the review heuristic.
- Engram mirror topic: `odd/destiny-workspace-foundations/tasks`.
- Branch: create `feature/destiny-workspace-foundations` from the completed architecture boundary before source implementation.

## Effective checks

- TDD mode: not configured; use ordinary functional checks and test-first behavior where practical, without claiming strict RED/GREEN evidence.
- Required commands:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace`
  - `cargo test --workspace --all-features`
  - `cargo run -p destiny-cli -- --help`
  - `cargo run -p destiny-mcp -- --help`
- Manual structural checks: dependency direction and absence of secrets in diagnostics.

## Tasks

- [ ] **DWF-001 — Establish the feature boundary and validate toolchain choices**
  - Route: parent repository-state operations plus delegated read-only dependency/version validation if needed.
  - Outcome: create the feature branch, confirm the local Rust toolchain, and select current compatible foundational crates without introducing provider SDKs.
  - Check: record observed toolchain versions, selected dependency rationale, and clean starting boundary.
  - Evidence: pending.

- [ ] **DWF-002 — Scaffold the workspace and crate boundaries**
  - Route: delegated to `gentle-ai-worker`; multi-file write trigger.
  - Outcome: create the workspace and five planned crates with visible inward dependency direction and help-capable CLI/MCP binaries.
  - Check: workspace metadata/build checks plus both `--help` commands.
  - Evidence: pending.

- [ ] **DWF-003 — Implement configuration and sanitized error foundations**
  - Route: delegated to `gentle-ai-worker`; multi-file write trigger.
  - Outcome: add shared public-client configuration parsing/validation, exact callback URI checks, adapter-neutral typed errors, sanitized diagnostics, and focused tests.
  - Check: focused tests plus the full baseline command set and manual secret-redaction readback.
  - Evidence: pending.

- [ ] **DWF-004 — Verify and close WU-001**
  - Route: native review/assessment and delegated verification as required by the active RDD policy.
  - Outcome: independently verify the committed work-unit candidate, record any correction, and close the feature when all required checks are observed.
  - Check: final baseline commands, repository status, review outcome, and tracker evidence.
  - Evidence: pending.

## Acceptance criteria

- The repository is a Cargo workspace with the five planned crates and visible inward dependency direction.
- Configuration accepts the required public client/API key inputs, validates one exact case-sensitive callback URI, and has no client-secret path.
- Presentation boundaries receive only sanitized typed errors.
- Missing runtime configuration fails clearly without leaking values.
- CLI and MCP help render without credentials.
- All baseline commands pass offline, including all-features tests.
- Every completed task records observed checks and, when explicitly authorized, a Conventional Commit work-unit identity.

## Progress

- Architecture package and implementation plan are complete at the current branch boundary.
- WU-001 was selected as the next implementation feature.
- No Rust implementation files existed when this tracker was created.

## Next step

Complete DWF-001, then delegate DWF-002 as the first bounded multi-file implementation slice.
