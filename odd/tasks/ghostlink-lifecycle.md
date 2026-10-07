# Ghostlink installation lifecycle

## Objective and rationale
Expose `ghostlink update` followed by `ghostlink uninstall` as safe installation lifecycle commands. Reuse the official cargo-dist updater rather than duplicate release fetching. Keep both commands independent of Bungie configuration, login, SQLite initialization, and network providers.

## Authorized scope and decisions
- User authorized update first, then uninstall.
- User selected **Linux first**. Windows must fail safely with actionable external/manual instructions; no deferred helper in this iteration. Official Linux target is x86_64 GNU. No new platform claims.
- Update delegates to the validated sibling `ghostlink-update`, optionally forwarding `--version`, and preserves user state.
- Uninstall previews and confirms a narrowly owned cleanup plan. Provide `--dry-run` and `--yes`; preserve configuration, database and credentials. User explicitly selected **no purge in this iteration**. Preserve unknown and externally overridden files. Reuse existing integration/handler cleanup only after proving ownership; report skipped/partial results accurately.
- No auto-elevation, shell interpolation of user arguments, package-manager overwrite, recursive deletion of shared directories, or indiscriminate process termination.
- Source/manual/ambiguous installations refuse mutation and provide actionable instructions.
- Technical artifacts and CLI copy follow existing English conventions.
- RDD is clone-local off; do not enable or run native reviews. Follow read-only ASSESS plans after each writer.
- **Work-unit commits authorized (2026-10-07)** on feature/lifecycle-commands: four commits (agent cleanup → URI cleanup → update → uninstall) plus tracker. Push/PRs/publishing remain NOT authorized.
- Intermediate commits are verified with bounded resources (nice, -j2, sequential, temporary worktree and target under ~/.cache, removed afterwards) after a prior desktop slowdown during Rust builds.

## Verified foundations
- cargo-dist 0.32.0 receipts contain install_prefix, install_layout, binaries, source, provider, version and aliases. Product receipt excludes the separately installed updater.
- Receipt source must match github / Yoryoboy / ghostlink / ghostlink; provider source cargo-dist.
- Linux receipt uses XDG_CONFIG_HOME or HOME/.config. Axoupdater additionally honors AXOUPDATER_CONFIG_WORKING_DIR and AXOUPDATER_CONFIG_PATH; inspect exact semantics before reuse and pin the child to the validated receipt.
- Cargo-home layout puts files in install_prefix/bin. Do not identify ownership from directory name alone.
- Axoupdater v0.10.0 CLI primary source verifies `--version` and default latest stable behavior: https://raw.githubusercontent.com/axodotdev/axoupdater/v0.10.0/axoupdater-cli/src/bin/axoupdater/main.rs
- No claim of atomic paired binary update, rollback or unconditional integrity verification. Existing updater behavior does not prove those properties.
- Windows parent executable locking precludes naive synchronous update/self-removal.

## Tasks and acceptance
- [x] T1 — Implement Linux update and shared installation validation (verified; commit deferred pending user authorization).
  - Parse command/optional version, dispatch before runtime construction.
  - Validate receipt, install root, expected product binaries, symlink/filename boundaries and trusted sibling updater. Handle supported layouts or explicitly refuse others.
  - Launch via argument vectors; JSON output remains one result/error object and child diagnostics are bounded.
  - Windows refuses safely with explicit standalone updater instructions.
  - Tests observe RED then GREEN; cover argument parsing, invalid/missing receipt, wrong source/root, malicious paths, missing updater, child failure, JSON and no runtime initialization. Never run the real updater.
  - Docs describe actual behavior and limitations.
- [x] T2 — Add installation-scoped agent cleanup (independently verified; commit deferred).
  - Compare bundled skill files exactly; preserve modified/unknown files and symlinks.
  - OpenCode cleanup removes only an exact local MCP command for this installation, preserving unrelated config and backups.
  - Name-only Claude/Codex/Pi removal is not automatically safe; provide manual follow-up instructions rather than execute generic cleanup.
  - Test-first offline fixtures; generic agent uninstall behavior unchanged.
- [x] T3 — Add installation-scoped Linux URI handler cleanup (correction independently verified; commit deferred).
  - Require desktop entry executable ownership and safe file paths.
  - Restore recorded association only while the current default remains Ghostlink; preserve another selected default or ambiguous records.
  - Never delete shared MIME files; safe bounded reads/exclusive temporary writes for the scoped route.
  - Test-first offline fixtures; no actual desktop/portal updates.
- [x] T4 — Implement conservative Linux uninstall (independently verified; commit deferred).
  - Read-only plan precedes confirmation; noninteractive mutation requires --yes.
  - Remove only known receipt-owned binaries/updater/receipt after scoped integration cleanup; unknown/shared files preserved.
  - **No --purge in this iteration**, per explicit user decision. Preserve configuration, SQLite, credentials, runtime sockets and all external paths; list preserved locations and manual cleanup guidance.
  - Tests observe RED then GREEN; cover preservation, symlinks, ownership, dry-run, confirmation, partial failure, JSON and Windows refusal. Test only isolated fixtures/fakes.
  - Docs and embedded skill/reference describe actual behavior and manual Windows instructions.
- [x] T5 — Verify both commands and reconcile documentation (passed; platform/live checks explicitly excluded).
  - Follow each writer's ASSESS verification plan; independent verification when required.
  - Focused CLI and adapter regression tests, workspace tests, formatting and relevant lint checks.
  - Functional checks use fake executable/temporary fixtures only; no live update, uninstall, keyring mutation or user-integration changes.
  - Document every failed, skipped, pending or unavailable check, including Windows execution not available locally.
- [x] T6 — Create verified work-unit commits (each commit independently verified).
  - Commit order: agent cleanup (T2), URI handler cleanup (T3), Linux update (T1, shared installation validation without uninstall-only code), conservative uninstall (T4), then this tracker.
  - Each commit carries its own tests/docs; final committed tree must equal the verified working tree.
  - Each intermediate commit passes bounded cargo check plus its focused tests and skill_drift.
- [ ] T7 — Verify the official installer end to end in a disposable Linux environment (user-assisted; after T6). Deferred to the pre-release session: no release exists yet, and the owner continues other work first. Recorded under PR-011 pending items in `odd/tasks/ghostlink-public-release.md`.

## Progress and evidence
- Exploration complete; Linux-first decision recorded. Feature branch: feature/lifecycle-commands.
- T1 writer implementation observed: command parsing/early dispatch, reusable cargo-home validation, kernel lifecycle lock, bounded child streams, sanitized routing environment, JSON and Windows refusal.
- T1 observed RED: update unrecognized; GREEN: `cargo test -p ghostlink --test update` passed 10 tests; agents/handler parsing passed 4 tests; formatting and diff checks passed.
- Independent T1 verification: update 10/10, agents/handler 4/4, fmt, diff and clippy passed. skill_drift initially failed 1/5; documentation correction observed GREEN (skill_drift 5/5 and update 10/10). Independent correction confirmation passed all 15 skill_drift/update tests and diff check. T1 implementation and checks are complete; commit remains explicitly deferred. T2 adapter-only implementation is running. Writer changed approximately 1,014 added/2 removed lines including defensive tests; no new dependencies.
- ASSESS returned unassessable because intended untracked files lack a native declaration; fallback plan requires independent verifier. RDD remains off; no native review started.
- Known verification gap: skills/ghostlink/SKILL.md and exit-code reference were outside T1 writer surfaces and need synchronization if skill_drift fails. Windows and real release update untested by design.
- Uninstall exploration shows name-only generic MCP cleanup and unconditional URI-handler restoration are unsafe for installation-scoped cleanup. Ownership guards must precede reuse; modified/ambiguous files remain preserved. Do not blindly purge active SQLite files.
- T2 writer observed RED (missing module), GREEN then refactor: agents adapter tests 17/17; CLI agents/skill_drift 7/7; fmt/diff checks passed. Three surfaces, ~597 added lines; new scoped_cleanup planning/execution API. Independent verification passed adapters agents 17/17 and CLI agents/skill_drift 7/7 plus diff check; no concrete blockers. Temp-write failure injection remains a coverage gap, and hostile local races are not eliminated.
- T3 writer observed RED (missing scoped methods), GREEN 34/34 then refactor 38/38 uri_handler tests; handler parsing 2/2; fmt/diff passed. New scoped plan/execute leaves generic behavior unchanged. Independent verification observed 38/38 adapter and 2/2 parser tests passing but found a no-rewrite higher-priority default defect: unchanged unrelated mimeapps.list and missing/invalid prior record can remove the desktop while Ghostlink remains selected. Sole T4 writer authorized to add failing fake regression and fix ONLY scoped_cleanup.rs/uri_handler_tests.rs before completion. T4 writer separately observed regression RED (38 pass/1 fail) then correction GREEN (39/39). Final independent verification confirmed 39/39 handler tests and the CLI regression retaining desktop/CLI/receipt; blocker resolved. Real desktop behavior/non-Linux execution untested.
- T4 writer observed uninstall parse RED; final CLI GREEN 34/34 (15 uninstall, 10 update, 5 skill_drift, 4 agent/handler parsing). fmt/diff/clippy passed. Optional updater handling and no-write preview/cancel verified with fixtures. Native Windows and real lifecycle execution intentionally untested; final receipt/CLI removals are not transactional. Final independent verification passed; no concrete candidate blockers found.
- Final independent verification: CLI focused 34/34; URI handler 39/39; agents 17/17; workspace all-features 1,521 tests across 45 suites, zero failed/ignored. Workspace clippy all targets/all features with warnings denied, fmt, diff checks and both command help smoke checks passed.
- Historical failed checks: initial skill drift omission and no-rewrite handler defect were corrected and independently verified. No final check failures. Windows runtime, live release installation/self-removal and real desktop/keyring operations intentionally skipped/unavailable; no native review because RDD off. ASSESS stayed unassessable, so independent high-risk verification ran.
- Review size: approximately 3,744 authored changed lines including untracked source/tests/docs; implementation split into coherent units, but delivery needs careful review/slicing rather than a giant PR.
- Worktree was initially clean on main at c353045. Final HEAD is 772d0234538278280d0778f4aa5c8efbb821e60c on feature/lifecycle-commands. Read-only incident check found reflog main fast-forward to origin/main and checkout at 772d023; no lifecycle commits exist. Do not undo external Git activity. Verification ran against this final base.
- No user installation, credentials or integrations were changed.
- Work-unit commits: authorized 2026-10-07. Created on feature/lifecycle-commands over 772d023: `8a38846` feat(adapters): add installation-scoped agent cleanup (T2); `835d5f9` feat(adapters): add scoped Linux URI handler cleanup (T3); `bc2da22` feat(cli): add receipt-validated Linux update (T1); `872f551` feat(cli): add conservative Linux uninstall (T4). Final committed tree equals the verified working tree (19 files, +3669/−4). bc2da22 was hand-split from shared files (command/runner/lib, README, docs, skill, exit codes); `installation.rs` lands whole in T1 because `validate` builds on the shared receipt validator `validate_uninstall`.
- T6 independent bounded verification (nice -n 19, 2 jobs, sequential, temporary worktree under ~/.cache) PASSED at every commit: fmt and `cargo check --workspace --all-targets --all-features` at all four. 8a38846 agents 17/17; 835d5f9 uri_handler 39/39 + agents 17/17; bc2da22 update/skill_drift/agents/handler_parsing 19/19, clippy -D warnings, `update --help` smoke, `uninstall` rejected with exit 2 and no uninstall lifecycle docs; 872f551 CLI focused 34/34 and clippy. Full workspace suite not rerun per commit (final tree equals the tree verified with 1,521 tests). Temporary worktree, target and tmp removed by parent after child safety blocked cleanup.

## Next step
T6 done. User authorized push, PR and merge on 2026-10-07. T7 deferred to the rc release session (tracked in PR-011).

Previous note: Implementation and applicable Linux/offline checks complete. Await user delivery instructions (commits/PR/publishing not authorized); recommend coherent review slices due ~3,744 changed lines. Before release, verify official installer deployment in a disposable Linux environment; Windows automated lifecycle support and purge remain deferred. Uninstall was split into bounded ownership-cleanup units to avoid a single multi-area change; purge was explicitly deferred by the user.
