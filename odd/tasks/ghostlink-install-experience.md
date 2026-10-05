# Ghostlink install experience (CI + callback handler auto-detect)

## Authorization and scope

User-approved scope, 2026-10-05. The user stopped the delegated Pi session
after GIE-001; the coordinating session finishes the work with the normal ODD
flow (Gentle AI subagents for bounded writes and verification). Platforms: Linux and
Windows only; macOS is out of scope. This phase: (1) CI, (2) Linux handler
detection. The Windows handler adapter is a later phase.

Isolation: worktree `../ghostlink-install` on
`feature/ghostlink-install-experience` from `origin/main` `1e11393`. The main
checkout's `target/debug/ghostlink` is the installed desktop handler and must
not be rebuilt. `skills/` belongs to a parallel session.

Rules: English artifacts; never read `~/.config/ghostlink/config.env` or
secrets; never run live login; never modify the real handler association; no
push, PR or merge.

## Agreed behavior

- `ghostlink login --callback auto` becomes the default; `paste` and `handler`
  stay explicit overrides.
- auto: handler registered and pointing at the current executable -> handler.
  Missing or pointing elsewhere -> show the exact system changes and ask; yes ->
  install (recording the previous handler for restore) and use the handler; no
  -> paste with a notice.
- Never prompt when stdin or stdout is not a terminal, or with `--json`: paste
  with a notice.
- Uninstall path restores the previous handler.
- Detection and registration live behind a platform port
  (`UriHandlerRegistrar`) in `destiny-app`; the Linux adapter lives in
  `destiny-adapters` and is chosen only at the CLI composition root.
- Linux pitfalls detected and reported: contradictory `mimeapps.list`
  sections and the portal `desktop-used-apps` remembered choice.

## Decisions

- Uninstall surface: a small `ghostlink handler {status,install,uninstall}`
  subcommand group instead of `login --uninstall-handler`. Uninstalling is
  system configuration, not authorization; a flag on `login` would have to
  conflict with every other login flag and would hide the operation inside a
  command that otherwise talks to the provider. The group also gives agents
  and scripts a non-interactive, explicit install path (auto never prompts
  without a terminal) and a read-only diagnostic (`status`) that reports the
  Linux pitfalls.
- The previous-handler record keeps the existing runbook format and path:
  `~/.local/share/ghostlink/previous-uri-handler`, one desktop id plus newline
  (empty when none), mode `0600`. An existing record is never overwritten, so a
  reinstall keeps the original handler to restore.
- Windows compilation stays unverified locally (user decision): the host has
  a pacman Rust toolchain without `rustup` or a Windows target. The CI
  `windows-latest` job is the first real check, after the user pushes.
- The registrar port lives in `destiny-core` (`uri_handler.rs`), like every
  other port; `destiny-adapters` depends only on core, so a port in
  `destiny-app` could not be implemented there. The `auto` policy stays in
  `destiny-app` and re-exports the port.
- The Linux adapter refuses executable paths that would need escaping inside
  a quoted `Exec` (`"`, `` ` ``, `$`, `\`, `%`, controls) instead of escaping
  them; spaces and other reserved characters are double-quoted.
- Pitfalls are detected and reported with exact fix commands, not repaired
  automatically: the portal store is not a documented user-editable API and
  the `mimeapps.list` conflict is DE-specific.

## Tasks

- [x] GIE-001 Tracker, worktree, Engram mirror.
- [x] GIE-002 CI workflow (ubuntu + windows). Windows compile unverified
  locally (see Decisions).
- [x] GIE-003 App port and auto-resolution policy (test-first).
- [x] GIE-004 Linux registrar adapter (test-first, injected paths/runner).
- [x] GIE-005 CLI: `--callback auto` default, consent prompt, `handler`
  subcommand, composition.
- [x] GIE-006 README and packaging runbook.
- [x] GIE-007 Independent verification and closure.

## Progress

All tasks complete on `feature/ghostlink-install-experience`. Push, PR and
merge are the user's decisions. Next phase: Windows registrar adapter
(`HKCU\Software\Classes\ghostlink`).

## Evidence

- Commits: GIE-002 `c480ab6`; GIE-003 `a7d0829`, port move `d08b773`.
- GIE-004: RED observed (20 failing against a `todo!()` skeleton), GREEN
  `cargo test -p destiny-adapters --lib uri_handler` 20 passed; clippy
  `-p destiny-adapters --all-targets --all-features -D warnings` clean. Tests
  are `cfg(unix)` because fixtures use Unix absolute paths. Commit `5298e55`.
- GIE-005 (gentle-ai-worker): RED on parsing and behavior stubs; GREEN
  `cargo test -p destiny-cli --all-features` 187 passed (171 unit, 16
  integration); workspace fmt and clippy clean; `login --help` and
  `handler --help` render. Parent reviewed the composition root. Commit
  `2fdb636`.
- GIE-006 (gentle-ai-worker): README Login/Callback handler sections and
  runbook automated install/revert; commit `8474cd4`.
- GIE-007 (gentle-ai-verify): fmt, workspace clippy, build, `cargo test
  --workspace --all-features` 1334 passed / 0 failed, help renders,
  `git diff --check` clean. Two medium findings fixed test-first (RED 2
  failing, GREEN 22 passed): shell-quote paths rendered as copyable commands;
  tighten an existing loose previous-handler record to 0600. Commit `d2db7d4`.
- Delivery: rebased onto `origin/main` `2414b85`, PR #46. First Windows CI run
  compiled and failed one test; fixes `63b406c` (OAuth loopback read re-arms
  early socket timeouts until the shared deadline), `f16c7ab` (CI
  `--no-fail-fast`), `73ea07a` and `efb32ef` (Unix-path and reset-sensitive
  tests made portable). CI run 37339209583 green on ubuntu-latest and
  windows-latest.
- Unverified: no live desktop install or login was run.

## Follow-ups (Windows phase)

- The configuration file resolves only from `XDG_CONFIG_HOME`/`HOME`; Windows
  normally sets neither, so it needs `%APPDATA%` resolution.
- Windows registrar adapter (`HKCU\Software\Classes\ghostlink`).

- GIE-002: workflow reviewed; not run on GitHub (no push).
- GIE-003: `cargo test -p destiny-app --lib uri_handler` 12 passed; fmt clean;
  `cargo clippy -p destiny-app --all-targets --all-features -D warnings` clean.
