# Pending provider refresh outcome

## Authorization and scope

User approved (2026-10-04) a distinct outcome for mutations that the provider
confirmed but whose post-action profile is still the pre-dispatch revision.
Branch feature/equip-item; work-unit commits authorized; no push/PR; no live
calls without separate authorization.

## Evidence

Live equip Antiope-D (see ghostlink-http-affinity.md): transfer and equip
provider-confirmed, immediate refresh and +~20s sync returned revision 8
(pre-mutation content); sync ~16 min later revision 9 with the item equipped.
Shared cookie affinity did not change this. Bungie GetProfile serves stale data
for a while after mutations; window unmeasured.

## Design

- New `MoveExecutionOutcome` variant; status `applied_pending_provider_refresh`.
- Qualifies only when every planned step was attempted with outcome
  `Completed`, the PostAction refresh succeeded with revision equal to the
  execution ticket's pre-dispatch revision (content-identical; SQLite dedupe),
  and the item is not observed at the verified target.
- Unchanged: `Uncertain`, `AuthenticationRefused`, partial plans (Incomplete),
  refresh failure, unresolved item, changed revision not matching the target
  (`verification_failed` Elsewhere), VerifiedNoOp.
- Never presented as verified success: text says applied per provider,
  unconfirmed; do not retry; run `ghostlink sync` later.
- CLI exit code 43. MCP `is_error = true` (user decision). MCP `move_item`
  gains the same variant (public schema change).
- No polling, sleeps or retries.

## Tasks

Regrouped before the first write: a new outcome variant breaks exhaustive
matches in destiny-output and MCP, so core and consumers land together.

- [x] PV-001 Outcome end-to-end: thread ticket revision into verify_attempt,
  strict qualification, shared output status, CLI exit 43 + sync hint, MCP
  move_item variant/view and equip is_error=true; unit tests (stale,
  changed-revision contradiction, Uncertain, AuthenticationRefused, partial)
  for move and equip plus output/exit/MCP view tests.
  RED: destiny-app pending_provider_refresh and equip_verified_execution_scenarios
  failed with Elsewhere; GREEN: mutation tests 100 pass, MCP pending 2, CLI
  equip_pipeline 5 (needs destiny-app/test-support feature standalone).
  Workspace default/all-features tests, all-features clippy, fmt, diff check
  PASS (writer-run). Flipped tests: adapters transfer journey unchanged-profile
  case now expects pending; MCP contradiction fixture now changes character
  content to keep Elsewhere coverage. move_item pending also is_error=true.
  Commit `d25f8d7`.
- [x] PV-002 Journeys and parity: adapters SQLite (retained revision ->
  pending; changed revision wrong state -> verification_failed), offline
  fixture scenarios (stale + real contradiction), CLI/MCP parity and stdio.
  New: confirmed_transfer_and_equip_with_stale_profile_are_pending,
  confirmed_equip_with_changed_wrong_state_is_elsewhere,
  changed_post_action_revision_with_wrong_location_is_elsewhere,
  move_item_reports_pending_provider_refresh_for_confirmed_stale_profile;
  fixtures equip-provider-stale / move-provider-stale. RED: parity saw
  verified_success, stdio move saw isError=false; GREEN after fixtures.
  Journeys 31, MCP equip+stdio 20; workspace default/all-features, clippy,
  fmt, diff check PASS (writer-run). No production change. Commit `6055a3a`.
- [x] PV-003 Docs (README, architecture), independent review, full offline checks.
  Docs commit `437a55f`. Independent verify PASS: fmt, clippy default/all-features,
  workspace tests 1287 / all-features 1302, diff check. Judge A: major race --
  another process publishing between refresh and item lookup could yield pending
  for a real contradiction; fixed by post-lookup revision bracketing (RED then
  GREEN, regression execute_move_verified_publication_between_refresh_and_lookup
  _is_not_pending), re-judgment verified. Judge B: minor lost no-error assertion
  on changed-revision journey, restored. Fix commit `bdfcf1e`; post-fix default
  workspace 1288 passed (parent), all-features + clippy + fmt PASS (writer-run).
  Concurrency proven through repository seam, not two SQLite connections.

## Status

All tasks done; no push/PR performed.

Live confirmation (user authorized, on 45dacca, rebuilt binary): pre-state
forced sync revision 9, Antiope-D equipped, Ghost Primus on Hunter unequipped.
One call `ghostlink --json equip 6917529045285107589 --character
2305843009300568190` at 1791161812: single equip step completed, refresh
refreshed, observed Hunter/equipped=false, status
applied_pending_provider_refresh, exit 43. No retry. Next-day forced sync
(checked_at 1791203045) revision 10: both items in Vault, unequipped, because
the user played and changed equipment overnight. Ghost Primus convergence is
therefore unconfirmable; Antiope-D convergence (revision 9) remains the
confirmed live case.

## Checks and delivery

CARGO_NET_OFFLINE=true, --locked, persistent target/. Test-first RED->GREEN per
task; fmt, clippy all-features, workspace default/all-features tests.
One work-unit commit per task with commit ref recorded here.
