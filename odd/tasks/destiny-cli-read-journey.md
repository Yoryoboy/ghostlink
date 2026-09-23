# Destiny CLI Read Journey

## Objective

Implement WU-008 from `docs/mvp-implementation-plan.md`: a thin read-only CLI over the shared WU-007 query services, with policy-driven freshness, concise human output, bounded JSON output, and stable typed exit behavior.

## Problem

The workspace has complete shared `list_characters`, `search_inventory`, and `inspect_item` services, but `destiny-cli` only validates startup configuration. Users cannot invoke the read journey, select machine-readable output, or receive stable process outcomes without writing adapter-specific logic.

## Scope and decisions

- Add `list-characters`, `search-inventory`, and `inspect-item` subcommands.
- Keep authorization and freshness decisions in shared application services. The CLI passes a freshness policy/current time into WU-007 and never reads repositories, SQLite, or Bungie directly.
- Add one narrow CLI runner boundary that accepts the shared application read interface, enabling deterministic offline integration tests without provider credentials.
- Keep definition hashes and instance IDs distinct. `inspect-item` accepts only a concrete instance ID. Search may filter definitions by text/type/tag and instances by exact perk hash, character ID, or matched definition hashes; it never silently chooses an ambiguous instance.
- Human output is concise line-oriented text with a metadata footer (`revision`, `checked_at`, `freshness`). Empty/missing outcomes are explicit. IDs are shown only where needed to make a result actionable.
- JSON output is selected by global `--json`. Each success is one bounded JSON object with a stable `kind`, bounded arrays, separate `definition`/`instance` objects, and metadata. No raw provider response, credentials, headers, or membership ID is serialized.
- CLI validation happens before service calls. Query bounds remain the shared WU-007 bounds.
- Automated checks are offline. Browser authorization remains the existing application/adapter boundary and is not reimplemented in this unit.
- Technical artifacts are English.
- Delivery strategy: `ask-on-risk`; each cohesive task closes with a Conventional Commit and tests/docs.

## Output contract

### Human

- `list-characters`: heading/count, then one line per character containing character ID and available normalized display fields; metadata footer.
- `search-inventory`: definition candidates and concrete instance hits in service order, with definition hash and instance ID clearly labeled; metadata footer.
- `inspect-item`: exact definition and instance state, location, quantity/equipped/locked/power/perk summary, and metadata; missing is a typed not-found outcome rather than a successful empty object.
- Errors are a single sanitized line on stderr prefixed with `error:`. No debug rendering is used.

### JSON

- Exactly one object on stdout for success; no human prose is mixed in.
- Stable top-level `kind` values: `character_list`, `inventory_search`, `item_inspection`.
- Metadata includes only revision, checked timestamp, source, and freshness.
- Search arrays remain bounded by the validated shared query limits.
- Errors are one sanitized object on stderr: `{\"error\":{\"code\":\"...\",\"message\":\"...\"}}`.

## Error and exit-code matrix

| Outcome/category | Stable code | Exit |
|---|---|---:|
| success | `ok` | 0 |
| CLI/shared request validation | `invalid_request` | 2 |
| authorization required/denied/in progress/already active | `authorization_required` / `authorization_denied` / `authorization_state` | 10 |
| reauthorization/provider authentication | `reauthorization_required` | 11 |
| provider rate limit | `provider_rate_limit` | 20 |
| provider unavailable/callback timeout | `provider_unavailable` | 21 |
| stale state/ambiguous match | `stale_state` / `ambiguous_match` | 30 / 31 |
| inspected instance missing | `item_not_found` | 32 |
| unsupported operation | `unsupported_operation` | 40 |
| partial failure/verification failure | `partial_failure` / `verification_failed` | 41 / 42 |
| credential store/local storage/configuration | `credential_store_unavailable` / `local_storage` / `configuration_error` | 50 / 51 / 52 |
| internal clock/identifier exhaustion | `internal_error` | 70 |

The matrix is exhaustive over the current `ErrorCategory`; tests must fail when a new category is not mapped.

## Exact edit surfaces

- `odd/tasks/destiny-cli-read-journey.md` — durable tracker and evidence.
- `crates/destiny-app/src/lib.rs` — presentation-safe re-exports only; no new CLI behavior.
- `crates/destiny-cli/Cargo.toml` — JSON/runtime test dependencies as required.
- `crates/destiny-cli/src/lib.rs` plus focused modules under `src/` — parser, shared-service dispatch, rendering, and typed exits.
- `crates/destiny-cli/src/main.rs` — minimal process boundary only.
- `crates/destiny-cli/tests/` — offline fixture/read-journey integration and snapshots/assertions.
- `README.md` — command/output examples and offline/manual boundary.

No other crate or path is in scope unless a discovered compile-time requirement is recorded here before editing it.

## Tasks

- [x] **DCR-001 — Establish the CLI read boundary**
  - Outcome: mapped WU-007 contracts and created this tracker before any source write, including exact surfaces, output contracts, error matrix, checks, and rollback.
  - Check: tracker and Engram mirror readback; `git diff --check`.
  - Rollback: remove only this tracker and its Engram mirror.
  - Evidence: pending boundary commit.

- [ ] **DCR-002 — Add CLI command, rendering, and typed-exit contracts**
  - Outcome: parse all three read commands and JSON selection; validate/convert inputs; render bounded shared results; exhaustively map application errors and missing inspection to stable stderr/exit outcomes.
  - Checks: focused parser/render/error tests, fmt, clippy, diff check.
  - Runtime harness: invoke parser/renderer with deterministic shared records; no external runtime boundary exists yet.
  - Rollback: remove CLI library/modules and presentation-safe app re-exports, preserving the original binary startup behavior.
  - Commit: Conventional Commit with tests.

- [ ] **DCR-003 — Dispatch reads through the shared freshness-aware service**
  - Outcome: one thin command runner calls only shared application query operations, passing policy/time without reproducing freshness or authorization rules; verify no call occurs after invalid input and all results retain shared metadata.
  - Checks: focused runner tests plus CLI crate tests/clippy.
  - Runtime harness: deterministic fake service records list/search/inspect requests and returns fresh/stale-refreshed outcomes.
  - Rollback: remove runner boundary and tests while retaining parser/render contracts.
  - Commit: Conventional Commit with tests/docs.

- [ ] **DCR-004 — Prove the offline human and JSON journey**
  - Outcome: integration-test list/search/inspect, fresh reuse, stale refresh, ambiguity, missing item, typed failures, stdout/stderr separation, bounds, and secret absence using deterministic fixtures; document user commands and manual browser/reauthorization boundary.
  - Checks: focused integration suite and full workspace baseline.
  - Runtime harness: execute the CLI application boundary end-to-end with fixture services in both human and JSON modes; no live Bungie account.
  - Rollback: remove the integration target and README additions without affecting CLI contracts.
  - Commit: Conventional Commit with tests/docs.

- [ ] **DCR-005 — Verify and close WU-008**
  - Outcome: independently verify the committed feature range, record exact checks, workload, limitations, rollback boundary, and commit identities.
  - Checks: fmt; workspace check/clippy/tests default and all features; CLI/MCP help; focused CLI journey; `git diff --check`; clean status.
  - Runtime harness: documented deterministic read journey plus manual limitation for system-browser authorization.
  - Rollback: reset this feature branch to `2b6c00159ebc574d1e97d9ffc490244d04f1ca70`.
  - Commit: Conventional Commit containing tracker closure only.

## Acceptance criteria

- All three read commands dispatch through shared application services and never access adapters/repositories directly.
- Policy-driven fresh reuse and stale refresh remain WU-007 behavior, proven through the CLI journey without duplicated freshness logic.
- Human and JSON results are deterministic, bounded, sanitized, and preserve definition/instance identity.
- Every current typed application error and missing inspection has a stable code and exit status.
- Invalid arguments cause no service call; errors never echo secrets or raw provider material.
- Offline tests cover success, stale refresh, ambiguity, missing, and typed failure in both output modes.
- Every completed task records checks, runtime evidence or explicit N/A, rollback, and commit identity.

## Progress

- Base `2b6c00159ebc574d1e97d9ffc490244d04f1ca70` is the closed WU-007 boundary.
- WU-007 already owns query validation, synchronization policy, revision/freshness metadata, deterministic ordering, bounds, and ambiguity-safe results.
- No live provider or credential dependency is required for automated WU-008 verification.
