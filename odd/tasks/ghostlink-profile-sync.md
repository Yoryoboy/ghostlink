# Explicit profile sync

## Authorization and scope

User approved `ghostlink sync` and an independent profile read to investigate
failed Ghost Primus equip verification. Reuse existing Forced sync and runtime.
No game mutations, mutation retries, manifest changes, dependencies or commits.
Preserve all uncommitted equip work on `feature/equip-item`.

## Acceptance

- Argument-free `sync`, global JSON supported in either position.
- Forced profile acquisition even when ordinary projection is fresh.
- Report fetched/publication/revision/previous revision/checked_at truthfully;
  unchanged revision is not evidence of unchanged game state or origin freshness.
- Existing sanitized failures and process exit conventions retained.
- Test-first observed runtime RED/GREEN; offline target/ checks.

## Tasks

- [ ] PS-001 Implement CLI forced profile sync with tests and concise README.
  Implementation exists; acceptance blocked by checks. Surfaces: command.rs,
  runtime.rs, runner.rs, output.rs under crates/destiny-cli/src and README.md.
  No shared QueryOperations expansion. Observed runtime RED: sync rejected at
  process boundary (162 passed, 1 failed); all-features GREEN 1285. Forced fresh
  acquisition, initial/unchanged/different revisions, failure preservation and
  exact bounded output covered. fmt/clippy/diff pass. Focused default CLI runs
  fail compilation on login.rs feature-gated test_support; default workspace
  fails OAuth callback timeout (UnsupportedOperation vs CallbackTimeout).
  Independent sync review PASS; workspace 1271 / all-features 1285, fmt/clippy/
  diff pass. Isolated CLI default compilation failure proven preexisting via
  unchanged HEAD login/test_support gate; feature-forwarded focused tests pass
  command 25 / runner 41 / runtime 22 / output 51; app sync 16. OAuth targeted 1
  and both workspace runs pass, historical failure cause unresolved. PS-001
  full isolated-test closure remains pending this preexisting blocker; no edits
  outside authorized surfaces. No mutation retries. Commit `64c8227`.
- [x] PS-002 Independently verify command and final functional checks.
  Bounded PASS (`muucuzer-l-n0gi`), all results and known isolated-test limitation
  above. No introduced sync defect found; safe for authorized read-only use.
- [x] PS-003 Run one authorized live sync and exact Ghost Primus inspection.
  PASS (`muuczg1d-m-hzbq`): both commands exit 0. Forced acquisition committed
  revision 8 (previous 7), checked_at 1791150892. Exact instance
  `6917529045285107589` observed on Hunter `2305843009300568190`, equipped=true.
  Inspection uses that published projection, not a second provider acquisition.
  No mutation retry. Historical immediate equip verification remains FAIL;
  cause of earlier contradictory profile remains unknown.

Follow-up (user decision 2026-10-05): PS-001 merges as delivered; the
pre-existing standalone `destiny-cli` test compilation failure (feature-gated
`destiny_app::test_support` in `src/login.rs` and `tests/equip_pipeline.rs`,
present on main) is a known limitation to fix in a separate work unit.

## Evidence

Scout `muucdwtt-j-itpr`: Forced sync bypasses ordinary freshness; Refreshed means
successful acquisition/publication, not necessarily changed revision. Existing
SQLite dedupe preserves revision while updating metadata. Test commands use
CARGO_NET_OFFLINE=true and persistent target/.

## Delivery

No commits/push/PR under existing user constraint. EQ-007 remains blocked by
historical live verification failure; no cause inferred yet.
