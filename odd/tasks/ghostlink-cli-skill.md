# Agent-facing Ghostlink CLI skill

## Authorization and scope

User decisions (2026-10-05): build a skill, versioned in the repository, that
teaches shell-capable agents to operate Ghostlink through the CLI only (MCP is
out of scope). The skill holds judgment (intent → command, freshness, output
contract, mutation safety) and defers flag details to `ghostlink <cmd> --help`.
A drift test keeps the skill aligned with the clap command surface.
Branch: `feature/ghostlink-cli-skill`. No push or PR without user approval.

## Design

- Skill file: `skills/ghostlink/SKILL.md`, Agent Skills frontmatter, under
  ~150 lines.
- Drift contract: every inline code span starting with `ghostlink <cmd>` names a
  real subcommand, every real subcommand (except `help`) appears at least once,
  and every `--flag` in such a span exists on that subcommand or globally.
- Drift test: `crates/destiny-cli/tests/skill_drift.rs`, reading the skill
  through `CARGO_MANIFEST_DIR` and the surface through `Cli::command()`.

## Tasks

- [x] SK-001 RED: drift test against the missing skill. Observed RED: 3 contract
  tests failed reading `skills/ghostlink/SKILL.md`; parser test passed. Hidden
  subcommands (`callback`) excluded from coverage.
- [x] SK-002 GREEN: `skills/ghostlink/SKILL.md` plus
  `references/exit-codes.md`; 4 drift tests pass. Mutation check: renamed
  command, removed command and misspelled flags each failed the right test.
- [x] SK-003 Validate content. Independent verifier found: failures go to
  stderr, two failure shapes for equip, missing exits (2 name codes, 33),
  variable refusal exits, login not always opening a browser, exit 21 covering
  callback timeout, `refused` possibly following a completed transfer, login's
  extra stdout object, and a drift hole accepting root-only flags on
  subcommands (fixed with observed RED then GREEN). All corrected; re-verified.
- [x] SK-004 Linked from `README.md` and `docs/development.md`. Verifier: fmt,
  clippy `-D warnings`, `git diff --check` pass; workspace 1286 passed, 0
  failed. Final edits were markdown only; drift test 5 passed afterwards.

## Follow-ups

- The drift test covers `SKILL.md` only; `references/exit-codes.md` is not
  checked against `category_outcome`.
