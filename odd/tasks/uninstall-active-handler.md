# Uninstall with active ghostlink:// handler (issue #67) — DONE

`ghostlink uninstall --yes` exits 41 with nothing removed when the
`ghostlink://` handler is the active default, which is exactly the state
`ghostlink login` leaves behind.

## Scope and decisions (user, 2026-10-08)

- Worktree `../ghostlink-uninstall-handler`, branch
  `fix/uninstall-active-handler` from `origin/main` at `678a623`.
- User authorized fix, commit, push and merge.
- Out of scope: JSON report stream (stderr for error statuses is the existing
  convention) and the intentional lifecycle lock file.

## Tasks

- [x] **T1 — Root cause + fix.** Reproduce with fixtures that mirror the live
  state (desktop entry owned, record `google-chrome.desktop`, scheme lines in
  `[Added Associations]` and `[Default Applications]`). Fix the false skip if
  it is a defect. A handler that still cannot be cleared safely must not
  block binary/receipt removal: proceed, keep the handler, report partial
  with exact manual steps (`xdg-mime default <id> x-scheme-handler/ghostlink`
  or remove the line from `mimeapps.list`). `--dry-run` must warn about it.
  Docs/skill/exit-code reference updated.
- [x] **T2 — Deliver.** Commit, push, PR, merge.

## Evidence

- Root cause: the adapter correctly skips when Ghostlink is the active
  default but no owned `mimeapps.list` line selects it; the CLI treated that
  read-only skip like a mutation failure and returned exit 41 before removing
  anything. Adapter guards unchanged.
- Fix: skips no longer block removal of receipt-owned files; handler files
  preserved; partial (41) with populated `removed` and concrete manual steps.
  Mutation errors/stale snapshots still abort. Dry-run reports
  `expected_status: "partial"` plus a warning (JSON and human).
- RED: uninstall 14 pass/1 fail (removed 0, expected 3), then 14/3 with
  preview-warning and foreign-skip cases. GREEN: uninstall 17/17,
  uri_handler 39/39, skill_drift 5/5, fmt, clippy `-D warnings`,
  `git diff --check`.
- Commit: `ae2d79a` fix(cli): remove installation files when handler cleanup
  is skipped.
- Not done: live desktop verification (rc.1 binary no longer installed).
