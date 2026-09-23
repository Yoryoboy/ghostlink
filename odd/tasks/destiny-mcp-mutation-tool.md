# Destiny MCP Mutation Tool

## Status

WU-012 ACTIVE on `feature/destiny-mcp-mutation-tool`, based on WU-011 `8fe817b`.

## Objective

Expose one default-enabled semantic `move_item` MCP tool over the shared WU-009 planner and WU-010 executor. One explicit MCP request is sufficient intent authorization. The service must still resolve semantic identity, reject ambiguity, perform fresh revision-bound preflight, use the minimum supported transfer calls, preserve bounded partial-failure evidence, refresh state, and verify the destination. There is no preview tool, confirmation token/value, approval ceremony, or second MCP call.

## Problem and rationale

WU-009 and WU-010 provide the safe shared mutation contracts and execution behavior, while WU-011 provides the stdio MCP server and production read runtime. Their branches diverged after WU-007. WU-012 must integrate them without reverting WU-011's complete-credential persistence or leaking provider/runtime details into MCP contracts.

## Scope

- Integrate WU-010 `a167540` into the WU-011 base while preserving both contract sets.
- Add bounded, sanitized MCP mutation input/output contracts.
- Add an adapter-free MCP mutation service seam over application behavior.
- Register and dispatch exactly one `move_item` tool through stdio.
- Compose mutation dependencies in the product binary and prove the complete single-call journey offline.
- Preserve all existing read-tool behavior and default-enabled mutation behavior.

## Constraints and non-goals

- No preview, confirmation token/value, approval ceremony, or two-step protocol.
- No caller-supplied provider membership identity or credentials.
- No adapter-generated authority or bypass of planner, preflight, policy, refresh, or verification.
- No new mutation semantics, SQLite migrations, CLI changes, HTTP MCP transport, or model-specific behavior.
- MCP library code remains adapter-free; production adapters stay in binary composition.
- Technical artifacts remain in English.
- No push, pull request, merge to the default branch, or release without a separate user decision.

## Delivery strategy and forecast

- Strategy: `stacked-to-main` (selected by the user).
- Forecast: approximately 1,400–2,600 authored changed lines, excluding generated lockfile churn.
- Planned slices:
  - Slice A: DMM-001 integration boundary.
  - Slice B: DMM-002 and DMM-003 contracts/service/server.
  - Slice C: DMM-004 production composition and stdio journey.
  - Closure: DMM-005 evidence-only closeout.
- Per-task ~400 authored changed lines is advisory only; coherent behavior, tests, and clarity take priority.

## TDD and routing

- Effective TDD mode: not explicitly enabled; ordinary behavior-first functional checks apply.
- Route: delegated direct implementation.
- Trigger evidence: understanding and implementation span 4+ files; all non-trivial multi-file writes use `gentle-ai-worker`.
- Verification: writer runs authorized focused checks; RDD is off, so native assessment determines whether separate verification is required. Final whole-range verification uses `gentle-ai-verify`.

## Tasks

- [ ] **DMM-001 — Establish the integration boundary** *(in progress)*
  - Merge WU-010 `a167540` into the WU-011-based feature branch.
  - Preserve WU-010 mutation/transfer contracts and WU-011 complete `AccessCredential` persistence, `SystemClock`, `ReadRuntimeConfig`, and MCP read server.
  - Resolve additive exports/test-support conflicts and regenerate `Cargo.lock` only as required.
  - Acceptance: both dependency trees compile together; no material-only `CredentialStore` contract remains; existing workspace behavior remains green.
  - Checks: formatting, workspace check/tests default and all-features, clippy default and all-features, `git diff --check`.
  - Commit evidence: pending.
  - Review evidence: pending.

- [ ] **DMM-002 — Add bounded MCP mutation contracts and service seam**
  - Add semantic move arguments for an instance ID or search text plus a Vault/exact-character/character-label destination.
  - Add bounded sanitized output views for effects, step evidence, final locations, and typed failures.
  - Compose planner/executor behavior behind an adapter-free MCP service seam and derive membership internally.
  - Acceptance: one invocation performs plan → fresh preflight → ordered execution → refresh → verification; missing/ambiguous/stale state fails closed; partial/uncertain outcomes remain honest; no secrets or raw provider payloads cross MCP.
  - Checks: focused MCP unit/contract tests, manifest dependency-boundary test, formatting, clippy, `git diff --check`.
  - Commit evidence: pending.
  - Review evidence: pending.

- [ ] **DMM-003 — Expose `move_item` through RMCP stdio**
  - Register `move_item` when the mutation seam is configured while retaining the three existing read tools.
  - Dispatch one `tools/call` directly through the mutation seam under existing serialization and bounded protocol errors.
  - Acceptance: discovery exposes four tools in product composition; read-only construction remains compatible; one call completes the semantic move without an approval artifact or second tool call.
  - Checks: focused server dispatch/discovery/serialization/error tests, formatting, clippy, `git diff --check`.
  - Commit evidence: pending.
  - Review evidence: pending.

- [ ] **DMM-004 — Compose production mutation and prove the stdio journey**
  - Construct the shared mutation service, production plan ID generation, and Bungie transfer gateway in the product binary.
  - Extend the offline fixture and child-process stdio tests with successful, repeated/already-satisfied, preflight-change, partial/incomplete, sanitized-failure, and sentinel-exclusion behavior as appropriate.
  - Document default-enabled host behavior only where existing project docs require it.
  - Acceptance: the normal product binary advertises mutation by default; missing credentials fail closed; one offline stdio `move_item` request proves planning, preflight, minimum calls, refresh, and destination verification without a second tool call.
  - Checks: `destiny-mcp` default/all-feature tests, child-process stdio journey, binary help/startup fail-closed checks, applicable workspace matrix, formatting, clippy, `git diff --check`.
  - Commit evidence: pending.
  - Review evidence: pending.

- [ ] **DMM-005 — Independently verify and close WU-012**
  - Verify the complete committed range against WU-012, AC-4/6/7/8/9, all exclusions, secret boundaries, and rollback boundary.
  - Reconcile every finding before closure and record exact observed evidence.
  - Acceptance: no blocker/high/medium finding remains; every acceptance criterion has direct evidence; worktree is clean after closure evidence is committed.
  - Checks: full workspace matrix, MCP focused matrix, binary smoke checks, `git diff --check`, committed-range/status inspection.
  - Commit evidence: pending.
  - Review evidence: pending.

## Acceptance criteria

1. MCP exposes exactly one mutation tool, `move_item`, alongside the existing read tools.
2. A single explicit request authorizes and completes the shared planner/executor flow; no preview, token, approval, or second call exists.
3. Semantic item/destination resolution rejects missing and ambiguous targets without remote mutation.
4. Fresh preflight, revision/current-state validation, minimum transfer calls, bounded step evidence, post-action refresh, and destination verification remain enforced by shared application behavior.
5. Repeated/already-satisfied requests and changed preflight state produce typed, honest outcomes without unsafe replay.
6. Partial, uncertain, transient, rejected, and verification-failure outcomes remain bounded and sanitized.
7. Provider membership and credentials are resolved internally and never accepted or emitted through MCP.
8. Mutation is enabled in normal product composition without breaking read-only library construction or existing read tools.
9. Offline stdio integration proves the complete one-call journey and secret exclusion.
10. Rollback can remove MCP mutation schemas/mapping/server wiring/tests without removing shared mutation behavior.

## Progress and evidence

- Read-only mapping completed before writes.
- User selected `stacked-to-main` delivery strategy.
- Created worktree `/home/yoryo/apps/ghostlink-wu-012` and branch `feature/destiny-mcp-mutation-tool` from WU-011 `8fe817b`.
- Engram mirror initialized as observation `5987` under `odd/destiny-mcp-mutation-tool/tasks`.
- DMM-001 is in progress.

## Next step

Integrate WU-010 `a167540` through a bounded delegated writer, then record exact checks and commit identity.
