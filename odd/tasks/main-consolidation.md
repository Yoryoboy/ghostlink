# Main Consolidation (WU-001–014)

## Status and intent

IN PROGRESS on `feature/main-consolidation` from clean WU-014 `37786e0`. Establish the first `main` branch locally and on GitHub, make it the GitHub default, then leave no feature branches or auxiliary worktrees. WU-015 is deferred to a later session. User explicitly authorized remote publication/default change and cleaning all six generated Rust `target/` caches (~30 GB); no production release or live Bungie work.

## Evidence and constraints

- Before consolidation there is no local or remote `main`/`master`. GitHub `Yoryoboy/ghostlink` is private, viewer has ADMIN, remote HEAD/default points to `feature/destiny-profile-sync-foundations` (`8a2250e`), and seven old feature branches are published; there are no open PRs.
- Of 18 original local feature branches, 17 tips are ancestors of WU-014 HEAD. WU-008 `ae09804` diverged at `2b6c001`; independent read-only verification found the entire CLI subtree and all seven WU-008-added paths byte-identical in WU-008 and WU-014 HEAD. An ancestry-only `merge -s ours --no-ff` must be labelled clearly as preservation of already-integrated content, not as reapplication of WU-008 code; verify its tree stays identical and original WU-008 tip becomes an ancestor before deleting that branch.
- Five auxiliary worktrees WU-008–012 have clean tracked status; ignored `.atl/` files (~12 KB each), `.codegraph/` in WU-009, and `target/` build caches total ~19 GB. Main `target/` is ~11 GB (debug incremental/dependencies); source/docs are ~2.3 MB and `.git` ~3 MB. User explicitly opted to remove all six generated `target/` caches and the five auxiliary worktree directories. Do not delete non-generated or dirty data; stop on an unexpected file.
- No force push. Publish local `main` and confirm remote identity before changing default; change GitHub default before deleting the seven old remote feature branches. Delete only exact audited branches, and use safe `git branch -d` after all become ancestors. Never delete the new `main` or other unknown refs. A failed/uncertain remote mutation is not blindly retried.
- TDD mode: not configured; this is Git/history maintenance with no application source changes. Verification runner: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` (WU-014 baseline 737 tests), and Git tree/ancestry/status checks. Run these before cleaning generated caches; afterward perform lightweight Git/GitHub checks, not a rebuild.
- Review forecast: tracker plus one ancestry-only merge and evidence updates; no production source diff. Rollback before publication: restore feature tip and remove newly created `main` only with explicit confirmation. After publication/default switch, report partial state and request direction before remote rollback.

## Tasks

- [x] **MCI-001 — Persist audit and consolidation tracker.** Route: inline Git state plus delegated read-only topology/assumption checks. Checks: tracker + full Engram mirror readback (`6029`), branch and remote inventory, clean worktrees, `git diff --check` exit 0. Commit: `8a90058` (`docs: plan main consolidation`).
- [x] **MCI-002 — Preserve WU-008 ancestry without altering product tree.** Route: parent Git integration after independent exact-tree challenge. Merge `742c70e` (`chore: preserve already integrated WU-008 history`) has parents `558ddfb` and `ae09804`; its Git tree before/after is exactly `8024ad0dbcca1ed228d493bb852dd7dd5d52538c`. WU-008 is now an ancestor; no source diff was applied. Rollback boundary: this ancestry-only merge (no product behavior to revert). Commit: `742c70e`.
- [ ] **MCI-003 — Verify integrated candidate and create local main.** Route: independent `gentle-ai-verify` for full offline commands; parent spot-check and Git status/ancestry. Create `main` at verified candidate tip and switch to it, without rewriting existing history. Commit: integration commit from MCI-002 (no unnecessary source edit).
- [ ] **MCI-004 — Publish and set GitHub default main.** Route: parent exact remote Git/gh commands. Push `main` normally, verify exact remote SHA, change GitHub default to main, then confirm `gh repo view` and remote HEAD. Record remote outcome. Commit: evidence tracker update if needed.
- [ ] **MCI-005 — Remove superseded refs and generated worktrees/caches.** Route: parent exact audited cleanup. Only after main is published/default and all local branch tips are ancestors: remove the five clean auxiliary worktrees with user-approved ignored build caches, delete old local feature branches with `-d`, delete only the seven enumerated remote feature branches, then clean main's generated `target/`. Preserve non-generated surprises and stop instead of forcing uncertain state. Commit: evidence tracker update if needed.
- [ ] **MCI-006 — Confirm main-only state and WU-015 handoff.** Route: read-only local and remote inventories, Git tree/HEAD/status and disk usage. Record failed/skipped/pending checks and final SHA, leave current branch `main` clean; no WU-015 branch until the next session. Commit: final tracker evidence commit on main if required.

## Progress

- Audit established WU-014 source baseline `37786e0`; tracker commits `8a90058` and `558ddfb`; tree-identical WU-008 ancestry merge `742c70e`. No `main` exists yet and the integration branch is clean.
- Worktree/cache deletion explicitly authorized by user after measuring ~30 GB; nothing deleted yet.

## Next step

Independently run offline suite and Git ancestry checks, then create local `main` at the verified tip.
