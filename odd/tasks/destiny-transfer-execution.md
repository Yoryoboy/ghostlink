# Destiny Transfer Execution

## Status

IN PROGRESS on `feature/destiny-transfer-execution`, based on closed WU-009 commit `dbf71b0`.

## Objective

Implement WU-010: a shared one-item Destiny transfer executor with mandatory fresh preflight, minimum ordered Bungie calls, durable step evidence, honest partial failure, mandatory post-action refresh, and destination verification.

## Problem and why

WU-009 can authorize, resolve, plan, and freshly preflight a semantic `move_item` request, but it intentionally has no remote mutation seam. WU-010 must cross that boundary without claiming multi-call atomicity or rollback, and must preserve enough evidence to tell success, partial completion, transient failure, and verification failure apart.

## Scope

- Add narrow provider-neutral transfer and execution contracts.
- Execute only a preflight-approved `MovePlan`, in its existing step order.
- Call Bungie's one-item transfer endpoint with the minimum supported request fields.
- Record completed and uncompleted steps without credentials, raw provider payloads, or user-identifying values.
- Force a post-action profile refresh after every attempted remote mutation sequence.
- Verify the concrete item destination against the refreshed projection.
- Refuse blind retry after an uncertain or partial attempt; require refreshed state and a new preflight.
- Prove direct, Vault-mediated, changed-state, partial-failure, and verification-failure paths offline.

## Constraints and non-goals

- No multi-item execution, equip, dismantle, locking framework, automatic rollback, or multi-call atomicity claim.
- No networked automated test and no committed live identifiers or credentials.
- No CLI or MCP mutation surface; those belong to WU-012.
- Explicit intent remains the authorization fact; no preview token, approval artifact, or second MCP call.
- Existing profile components remain acceptable only if tests prove they expose the item location and transferability facts needed here.
- An already-satisfied postcondition is a verified no-op only after mandatory fresh preflight proves the concrete item is already at the requested destination; no provider call is made.
- Retries are not performed inside this unit. Provider transient/rate-limit outcomes are typed and bounded, but callers must refresh and create a fresh preflight before another attempt.

## Official provider evidence (revalidated 2026-09-23)

Primary current Bungie API documentation:

- `POST /Destiny2/Actions/Items/TransferItem/` with required OAuth scope `MoveEquipDestinyItems`: https://bungie-net.github.io/multi/operation_post_Destiny2-TransferItem.html
- Request schema `Destiny.Requests.DestinyItemTransferRequest`: `itemReferenceHash` (`uint32`), `stackSize` (`int32`), `transferToVault` (`boolean`), instance identifier (`int64`), `characterId` (`int64`), and `membershipType` (`int32`): https://bungie-net.github.io/multi/schema_Destiny-Requests-DestinyItemTransferRequest.html
- Transfer status meanings (`CanTransfer`, `ItemIsEquipped`, `NotTransferrable`, `NoRoomInDestination`): https://bungie-net.github.io/multi/schema_Destiny-TransferStatuses.html
- Bungie's item-transfer FAQ documents character-to-character transfer as character → Vault → character, possible Vault stranding after a second-step failure, serialized provider actions, and the need to refresh because cached/affinity state can lag: https://github.com/Bungie-net/api/wiki/FAQ:-Item-Transfer,-what-can-and-can't-happen

Implementation must contract-test exact URL, trailing slash, JSON field names/types, bearer/API-key attachment, response-envelope classification, and the documented per-user throttle note. Documentation is provider-owned and time-bounded; live compatibility remains WU-015.

## TDD and routing

- TDD mode: not explicitly configured; use behavior-first tests and ordinary functional checks without claiming strict RED/GREEN evidence.
- Route: delegated direct ODD, not SDD.
- Mapping trigger: fired because WU-010 spans core, app, adapter, fixtures, and docs; completed by `gentle-ai-explore`.
- Multi-file writer trigger: each implementation slice routes through `gentle-ai-worker` with narrow edit surfaces.
- Verification trigger: after each writer, use native `assess`; RDD is off, so follow its risk-gated verifier plan.

## Delivery strategy

- Forecast: approximately 1,200–1,800 authored diff lines including behavior-first tests; generated files excluded.
- Review slices: (A) execution contracts, preflight, and provider-call sequencing; (B) partial-failure records, post-action refresh, verification, adapter, and offline journey.
- Strategy: `ask-on-risk`; the user selected `stacked-to-main` before the first work-unit commit.
- Slice A contains DTE-001 through DTE-003 at commits `268a7cc`, `5919cd6`, and `b07a044`; its evidence closure commit is pending.
- Slice B will branch from the Slice A evidence boundary and contain DTE-004 through DTE-006.
- No push, pull request, merge, or release is authorized.

## Tasks

- [x] **DTE-001 — Establish boundary and evidence**
  - Record scope, provider evidence, defaults, routing, delivery forecast, and rollback boundaries.
  - Checks: tracker readback; `git diff --check`.
  - Route: inline (single known documentation file).
  - Evidence: tracker and Engram mirror created; user selected `stacked-to-main`; `git diff --check` passed.
  - Commit: `268a7cc` (`chore: start Destiny transfer execution`).

- [x] **DTE-002 — Define narrow execution contracts**
  - Add provider-neutral transfer gateway/request/outcome and bounded execution/step evidence contracts.
  - Make retries impossible without refreshed state rather than embedding automatic retries.
  - Checks: focused core unit tests; formatting; clippy for touched crates.
  - Route: delegated writer (multi-file core contract slice).
  - Evidence: `TransferGateway`, `TransferRequest` (private fields, accessors, complete `DestinyMembership` identity), `TransferOutcome`, and `TransferRejection` added to `ports.rs` with an exhaustive `validate_endpoints` policy; `StepOutcome`, `StepRecord`, `ExecutionEvidence`, `ExecutionTicket`, and `MovePlan::already_satisfied` added to `mutation.rs`. `ExecutionTicket::after_fresh_preflight` takes an explicit `now` and enforces the plan window itself, and a debug test pins that the boundary request leaks no identifier value. `cargo test -p destiny-core mutation` (28 passed), `cargo test -p destiny-core` (63 passed), `cargo fmt --all -- --check`, `cargo clippy -p destiny-core --all-targets -- -D warnings`, and `git diff --check` all clean. No app execution, post-action refresh, adapter, fixture, CLI, or MCP work.
  - Commit: `5919cd6` (`feat: define transfer execution contracts`).

- [x] **DTE-003 — Execute preflight-approved ordered steps**
  - Reuse WU-009 mandatory preflight, handle already-satisfied fresh state as verified no-op, issue only ordered plan steps, and stop at first failure.
  - Preserve completed/uncompleted step evidence and distinguish deterministic, transient, uncertain, and partial failure.
  - Consume the single-use `ExecutionTicket` by value in the executor entry point, so one fresh preflight authorizes exactly one attempt sequence and no borrowing or replay path exists.
  - Checks: focused app tests for expiry/invalidation/order/failure/retry refusal.
  - Route: delegated writer (multi-file app slice).
  - Evidence: `MutationService::authorize_execution` validates intent first, rejects an expired plan or a wrong-membership request before any provider read, then reuses the WU-009 fresh-preflight acquisition and `evaluate_mutation` policy through extracted `fresh_preflight`/`evaluate_preflight` helpers, rejects a changed plan, returns `ExecutionAuthorization::VerifiedNoOp` with zero transfer calls when the refreshed projection already holds the concrete item at the plan destination, and otherwise mints exactly one `ExecutionTicket`. `MutationService::execute_move` consumes that ticket **by value**, rebuilds each `TransferRequest` from the committed projection (complete membership, definition hash, accepted quantity), issues only the ticket's ordered steps through an injected `TransferGateway`, stops at the first non-applying outcome, records post-dispatch uncertainty as `StepOutcome::Uncertain`, and returns bounded `ExecutionEvidence`. Verifier corrections applied: (Medium) every failure now returns a typed `ExecutionFailure { error, evidence }` carrying the bounded evidence observed so far plus `requires_post_action_refresh`, so a second-step pre-dispatch provider error preserves the first step's completed evidence instead of dropping it while a first-step pre-dispatch failure reports zero attempted steps as no remote mutation; (Low) expiry and membership mismatch are rejected before the forced preflight while intent stays first and the core ticket checks still run after refresh. Added `QueuedTransferGateway` to `test-support`; 19 behavior-first tests cover direct order, Vault-mediated order, the verified no-op, pre-refresh expiry/wrong-membership with zero provider calls, changed state, wrong item, missing resolution, first-step refusal and stop behavior, transient failure, uncertainty, partial completion, first- and second-step pre-dispatch errors, and a compile-time by-value signature pin. `cargo test -p destiny-app --features test-support mutation` (66 passed), `cargo test -p destiny-app --features test-support` (176 passed), `cargo test -p destiny-adapters --test mutation_planning_journey` (7 passed), `cargo fmt --all -- --check`, `cargo clippy -p destiny-app --all-targets --all-features -- -D warnings`, and `git diff --check` all clean. No post-action refresh, destination verification, adapter, fixture, CLI, or MCP work.
  - Commit: `b07a044` (`feat: execute ordered item transfers`).

- [ ] **DTE-004 — Refresh and verify every attempted sequence**
  - Force `ProfileSyncReason::PostAction` after provider calls, then verify concrete item destination from refreshed state.
  - Return honest verified, partial-failure, or verification-failed outcomes; never claim rollback.
  - Checks: focused app tests for post-refresh, changed state, Vault stranding, and verification failure.
  - Route: delegated writer.
  - Commit: pending.

- [ ] **DTE-005 — Implement Bungie transfer adapter**
  - Add a strict POST transport boundary and exact action allowlist/request mapping using the existing credential boundary.
  - Classify Bungie response envelopes without exposing provider messages, credentials, or raw payloads.
  - Checks: adapter contract tests for URL/body/headers/scope-shaped failures/transient classification.
  - Route: delegated writer.
  - Commit: pending.

- [ ] **DTE-006 — Prove the offline transfer journey**
  - Compose real mutation/app services, real SQLite projection, real Bungie profile/transfer adapters, and deterministic scripted transports.
  - Cover direct success, character-to-character success, second-step partial failure, changed preflight state, and failed post-action verification.
  - Checks: focused journey; workspace default/all-features tests; fmt/check/clippy; `git diff --check`.
  - Route: delegated writer, then independent verifier if native assessment requires it.
  - Commit: pending.

- [ ] **DTE-007 — Close WU-010**
  - Reconcile acceptance criteria, exact check evidence, review findings, authored-line count, commit boundaries, and next step for WU-012.
  - Checks: tracker/Engram/todo reconciliation and clean worktree.
  - Route: inline documentation plus routed verification.
  - Commit: pending.

## Acceptance criteria

1. Execution cannot start from an expired, stale, changed, ambiguous, or unauthorized plan/request.
2. Fresh preflight runs before any provider mutation; an already-satisfied concrete destination produces a verified no-op with zero provider calls.
3. Direct moves issue exactly one provider step; character-to-character moves issue exactly two ordered steps through Vault.
4. Every result contains bounded sanitized evidence of attempted, completed, failed, and unattempted steps.
5. A failed second step reports partial completion and possible Vault location; no rollback or atomicity is claimed.
6. Every attempted mutation sequence forces post-action refresh before final classification.
7. Success requires refreshed projection verification of the concrete item at the requested destination.
8. Retrying an uncertain/partial result without refresh and a new preflight is impossible through the public API.
9. Bungie adapter mapping is exact, credentials stay behind the adapter boundary, and automated checks stay offline.
10. The complete work unit passes focused and workspace checks and independent verification required by native risk assessment.

## Rollback boundaries

- Slice A: remove execution contracts and executor sequencing while preserving WU-009 planning/preflight.
- Slice B: remove transfer adapter, post-action verification, and offline execution journey while preserving Slice A contracts.
- Each behavior commit carries its own tests and documentation evidence.

## Progress and evidence

- 2026-09-23: resumed from Engram/project context and confirmed WU-009 closed at `dbf71b0`.
- 2026-09-23: created isolated worktree `/home/yoryo/apps/ghostlink-wu-010` on `feature/destiny-transfer-execution` from `dbf71b0`.
- 2026-09-23: delegated readiness mapping identified the missing transfer port, execution records, POST transport, refresh verification, and journey fixture.
- 2026-09-23: revalidated current official Bungie endpoint, request schema, transfer statuses, scope, throttle note, and Vault-mediated semantics from primary Bungie sources.
- 2026-09-23: user selected `stacked-to-main`; planned Slice A (DTE-001–003) and Slice B (DTE-004–006).
- 2026-09-23: DTE-001 tracker readback and `git diff --check` passed.
- 2026-09-23: DTE-002 execution contracts added: narrow provider-neutral transfer port, bounded sanitized step evidence, and a single-use revision-bound execution ticket that refuses a blind retry. Authored diff is ~999 lines (232 documentation, ~250 implementation, ~450 behavior-first tests), above the advisory ~400 target; correctness and test clarity were kept ahead of line count.
- 2026-09-23: DTE-002 verifier corrections applied: ticket minting enforces the plan window with an explicit current timestamp; `TransferRequest` carries the complete `DestinyMembership` with private fields and accessors; retry documentation now requires a new forced refresh plus a new plan only once state or binding changed; the DTE-003 by-value ticket consumption requirement is pinned in the contract and tracker; a direct-plan completion test was added.
- 2026-09-23: DTE-003 verifier corrections applied: (Medium) `execute_move` now returns `Result<ExecutionEvidence, ExecutionFailure>`, where `ExecutionFailure` carries the bounded evidence observed so far and `requires_post_action_refresh` marks every path DTE-004 must refresh; a second-step pre-dispatch error preserves the first step's completed evidence, while a first-step pre-dispatch error stays at zero attempts. (Low) `authorize_execution` validates intent, then rejects expiry and membership mismatch before the forced preflight, so neither spends a provider call, while the core ticket checks still run after the refresh.

## Next step

Commit the Slice A evidence update, then create stacked Slice B from that boundary for DTE-004.
