# rc.1 Live Validation Fixes (issue #66) — DONE (pending push/PR)

Fix the findings from live validation of `v0.1.0-rc.1` before testing continues
and before `0.1.0` ships. Parent tracker: `odd/tasks/ghostlink-public-release.md`
(PR-011).

## Scope and decisions (user, 2026-10-08)

- Branch `fix/rc1-live-validation` from `main` at `678a623`. One work-unit
  commit per finding; push/PR/merge are user decisions.
- F3 decision: human output omits unresolved modifiers; JSON keeps them as
  unresolved. No SQLite migration. Root cause verified against the live
  manifest: hash `1783825372` is not redacted but has an empty name (the only
  one of 351), and the decoder intentionally skips nameless modifiers.
- Test-first: `cargo test --workspace --all-features`; gates fmt, clippy
  `-D warnings`, `git diff --check`.

## Tasks

- [x] **F1 — MCP outputSchema root type.** `inspect_item` and `move_item` emit
  a root `oneOf` without `"type": "object"` (internally tagged enums through
  schemars 1.2 / rmcp 3.4); strict clients reject the whole `tools/list`.
  Emit `type: object` at the root; test that every tool's
  `outputSchema.type == "object"`.
  Done: `with_object_output_schema` helper on every tool (inserts root
  `type: object` when missing; `oneOf` and runtime output unchanged). RED:
  `every_tool_output_schema_declares_an_object_root` failed for inspect_item
  and move_item; GREEN plus stdio `tools/list` regression. 205 destiny-mcp
  tests in `unshare -rn`, clippy, fmt, diff --check. Release-build repro: all
  11 tools with an outputSchema report `type: object` (6 mutation tools have
  none, which is allowed). Commit evidence: `c79de0b`.
  Side finding (not fixed here): a debug-build `destiny-mcp` panics at
  startup (`Cannot drop a runtime in a context where blocking is not
  allowed`, `HttpTokenTransport::for_endpoint` via `configured_refresher`);
  release builds (rc.1 and this branch) are unaffected.
- [x] **F2 — search-inventory character selector.** `--character` is copied
  into an exact SQL predicate, so `hunter` silently returns 0 hits. Resolve it
  through the shared character selector (exact ID or unique class; invalid or
  ambiguous -> error). Check MCP parity.
  Done: the CLI runner lists characters and resolves the selector through
  `select_weekly_character` before searching; parse-level parity and MCP
  `search_inventory.character_id` (exact id) unchanged; help, docs/cli.md and
  the bundled skill updated. RED: hunter 0 hits, unknown/ambiguous exit 0;
  GREEN. Plain `cargo test --workspace --all-features` 1864 passed / 0 failed
  (`unshare -rn` workspace run hit callback-socket failures and wire-test
  hangs, timed out); clippy, fmt, diff --check. Commit evidence: `6ff5238`.
- [x] **F3 — weekly-reset unresolved modifiers.** Omit unresolved modifiers
  from human output; JSON unchanged. Matches `weekly`, which already hides
  unresolved modifiers in default human mode. RED: expected `modifiers: A`
  but got `A, unresolved #5`; new test (all-unresolved activity prints no
  modifiers line, JSON keeps hashes 5 and 8) failed; GREEN. Verifier: fmt,
  `cargo test --workspace --all-features` 1865 passed / 0 failed, clippy
  `-D warnings`, diff --check. Commit evidence: `3e09d74`.
- [x] **F4/F5 — README install notes.** Versioned URL for prereleases
  (`releases/latest` never resolves to a prerelease); open a new terminal or
  `source ~/.cargo/env` after install. Install dir verified from
  `dist-workspace.toml` (`install-path = "CARGO_HOME"`) and the rc.1
  installer. Passive docs, no test. Commit evidence: `af547b3`.

## Follow-ups

- Debug-build `destiny-mcp` startup panic (see F1 note); release unaffected.
- `unshare -rn cargo test --workspace` hangs on callback-socket/wire tests;
  plain runs pass.
- Live re-validation from a new rc (Pi MCP connect, `search-inventory
  --character hunter`, `weekly-reset`).
