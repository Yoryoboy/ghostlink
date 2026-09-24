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
- `crates/destiny-app/src/lib.rs` and `crates/destiny-app/src/queries.rs` — presentation-safe re-exports and a narrow shared query-operations trait implemented by `QueryService`; no CLI behavior.
- `crates/destiny-cli/Cargo.toml` — JSON/runtime test dependencies as required.
- `crates/destiny-cli/src/lib.rs` plus focused modules under `src/` — parser, shared-service dispatch, production runtime composition, rendering, and typed exits.
- `crates/destiny-cli/src/main.rs` — minimal process boundary only.
- `crates/destiny-cli/tests/` — focused CLI-only integration when a standalone harness is sufficient.
- `crates/destiny-adapters/Cargo.toml` and `crates/destiny-adapters/tests/query_journey.rs` — extend the existing real SQLite/gateway/query fixture harness through the CLI runner without duplicating its provider fixtures.
- `README.md` — command/output examples and offline/manual boundary.

No other crate or path is in scope unless a discovered compile-time requirement is recorded here before editing it.

## Tasks

- [x] **DCR-001 — Establish the CLI read boundary**
  - Outcome: mapped WU-007 contracts and created this tracker before any source write, including exact surfaces, output contracts, error matrix, checks, and rollback.
  - Check: tracker and Engram mirror readback; `git diff --check`.
  - Rollback: remove only this tracker and its Engram mirror.
  - Evidence: commit `932ca5d` (`chore: start Destiny CLI read journey`).

- [x] **DCR-002 — Add CLI command, rendering, and typed-exit contracts**
  - Outcome: added all three read commands, global JSON selection, validation/conversion into shared WU-007 request types, deterministic human/JSON rendering, explicit missing/ambiguous inspection handling, and an exhaustive typed application-error matrix. Presentation-safe application re-exports preserve the no-direct-core-dependency rule. Perk hashes remain accepted search inputs but are not rendered because domain policy marks them opaque.
  - Checks: `cargo fmt --all -- --check`; CLI 25 passed across 3 suites; app 23 passed across 2 suites; CLI all-target/all-feature clippy passed with `-D warnings`; `git diff --check` passed. Active LSP reported two stale false positives in `command.rs` contradicted by rustc tests and clippy.
  - Runtime harness: the CLI tests executed parsers and human/JSON renderers against deterministic shared records; no external runtime boundary exists yet. The production binary remains intentionally unwired until DCR-003.
  - Review workload: approximately 1,600 implementation-and-test lines across parser/contracts and renderers. The two modules are independently readable but jointly define one public CLI contract; tests remain beside behavior rather than being split by file type.
  - Rollback: remove CLI library/modules and revert the presentation-safe app re-exports, CLI manifest, and derived lockfile dependency while preserving the original binary startup behavior.
  - Independent verification: all focused checks passed; no blocker/high finding. One medium DCR-003 requirement was recorded above: normalize clap parse errors into the stable typed output contract.
  - Evidence: commit `e3aa9c9` (`feat: define Destiny CLI read contracts`).

- [x] **DCR-003 — Dispatch reads through the shared freshness-aware service**
  - Outcome: added the object-safe `QueryOperations` trait in `destiny-app`, implemented by `QueryService` and re-exported for presentation crates; it exposes only `list_characters`/`search_inventory`/`inspect_item` and retains the caller-supplied `FreshnessPolicy`/`Timestamp`. Added `destiny-cli`'s `runner` module: it validates the parsed command line before any service call, dispatches exactly once through the shared trait, renders the shared result (including metadata) in the selected mode, maps a missing inspection through the existing typed `item_not_found` outcome, and forwards caller-supplied freshness policy/time without duplicating any sync decision. The process boundary captures clap syntax/type failures and emits the sanitized `invalid_request`/2 contract in human or JSON mode, detecting global `--json` from raw tokens without echoing argv; clap's default error prose never reaches the process, while `--help`/`--version` pass through on stdout with a success status.
  - Checks: `cargo fmt --all -- --check` passed; `cargo test -p destiny-cli` 39 passed across 3 suites; `cargo test -p destiny-app --features test-support` 114 passed across 2 suites; `cargo clippy -p destiny-cli -p destiny-app --all-targets --all-features -- -D warnings` passed; `git diff --check` passed.
  - Runtime harness: a deterministic recording fake implements `QueryOperations`, records the exact request plus the freshness policy/time in call order, and returns canned shared records. Tests prove invalid clap input and invalid request bounds make zero service calls, valid list/search/inspect dispatch exactly once with the caller's policy/time, results retain shared metadata, and a missing inspection is the typed `item_not_found`/32 outcome.
  - Composition limitation: a production `QueryService` cannot yet be composed from public `destiny-app` foundations (`load_config` is crate-visible and no public function builds a service over the production adapters, which are not in `destiny-cli`'s dependency surface). No adapter or business logic was invented. `main.rs` is minimal: it calls the sanitized process boundary (`run_binary`), which keeps the sanitized startup diagnostic and reports the stable `unsupported_operation`/40 outcome for a read it cannot serve. `runner::tests::binary_boundary_documents_the_composition_limitation` records this boundary; DCR-004 wires the journey end-to-end with fixture services.
  - Rollback: remove the `runner` module and its tests, revert the `QueryOperations` trait and its re-export, and restore the minimal `main.rs`, while retaining the parser and render contracts.
  - Evidence: commit `28b09f2` (`feat: dispatch Destiny CLI reads`).

- [x] **DCR-004 — Prove the offline human and JSON journey**
  - Outcome: extended the existing real query journey in `crates/destiny-adapters/tests/query_journey.rs` with `destiny-cli` as a dev dependency and five focused tests that drive `destiny_cli::run` over the same real `QueryService`/`ProfileSyncService`/`BungieProfileGateway`/`SqliteStore` harness (scripted transport and clock, no duplicated provider fixtures). The journey covers human and JSON list/search/inspect, fresh reuse with no gateway call, a stale refresh exactly once with advanced metadata, explicit multiple candidates with no guessed instance, `--limit` bounding, a typed `item_not_found`/32 missing instance, paired definition/instance identity, stdout/stderr separation, a decoded provider failure rendered as the sanitized `invalid_request`/2 outcome, and credential/payload/membership sentinel absence on every stream. Added a `runner` unit test proving `--json` after `--` does not select JSON error mode. Documented the CLI syntax, output contract, and composition status in `README.md`.
  - Checks: `cargo fmt --all -- --check` passed; `cargo check -p destiny-cli -p destiny-adapters --all-targets --all-features` finished clean; `cargo test -p destiny-adapters --test query_journey` 14 passed; `cargo test -p destiny-cli` 40 passed (39 pre-existing + the new `--`-terminator test); `cargo clippy -p destiny-cli -p destiny-adapters --all-targets --all-features -- -D warnings` finished with exit 0; `git diff --check` passed.
  - Runtime harness: the real CLI runner executes over the composed real services with a scripted transport/clock; no live Bungie account or credential. The active lint pass reported `query_journey.rs`/`runner.rs` type diagnostics that rustc test compiles contradict (stale false positives on standard `&[&str; N]` unsizing, `serde_json::Value` indexing, and trait-object coercion); no code was rewritten to satisfy them.
  - Composition limitation: normal production composition remains blocked/pending the authorized foundational composition root; the offline harness proves behavior and is not a live-success claim.
  - Rollback: remove the five integration tests, the `destiny-cli` dev-dependency and its lockfile entry, the `runner` `--`-terminator test, and the README additions without affecting CLI contracts.
  - Commit: `05b258f` (`test: prove Destiny CLI read journey`).

- [x] **DCR-006 — Compose the production read runtime**
  - Outcome: added `destiny-adapters` to `destiny-cli` and a narrow `runtime` module composing the production read runtime behind an injectable `RuntimeFactory` seam. The process boundary parses and semantically validates the request first, then asks a factory for the runtime, which composes in the exact fail-fast order: `ReadRuntimeConfig::from_env`; `NativeCredentialStore::new`/`load` of the complete `AccessCredential` (missing => `AuthorizationRequired`); `SystemClock` with exactly one `now` reading; `HttpProfileTransport`; `ProfileApiKey` from the validated `ReadApiKey`; `BungieProfileGateway` with the configured definition version; `SqliteStore`; `ProfileSyncService`; `QueryService`. The single clock reading and the configured `FreshnessPolicy` form the `ReadFreshness` forwarded to the shared services. A valid command dispatches through the existing runner `dispatch` path, so the previous `unsupported_operation` fallback and its `binary_outcome` helper are gone; `main.rs` stays a two-line boundary. `destiny-app` now re-exports the credential-store contracts (`AccessCredential`, `AccessCredentialMetadata`, `CredentialMaterial`, `CredentialStore`) so the composition root needs no direct `destiny-core` dependency.
  - Checks: `cargo fmt --all -- --check` passed; `cargo test -p destiny-cli` 54 passed across 3 suites; `cargo test -p destiny-app --features test-support` 123 passed across 2 suites; `cargo test -p destiny-adapters` 234 passed across 4 suites; `cargo clippy -p destiny-cli --all-targets --all-features -- -D warnings` passed; `git diff --check` passed; the built binary returns `configuration_error`/52 for a valid command with no read environment and `--help` exits 0.
  - Runtime harness: scripted `ReadFoundations` prove the exact acquisition order and the sanitized mapping of configuration/credential/clock/transport/storage failures; a scripted `RuntimeFactory` proves a rejected parse or request performs zero runtime builds while a valid request dispatches once with the supplied service, policy, and time; a composed in-memory runtime dispatches through the shared runner and reports the provider's sanitized category. No live account, credential, keyring, or network is used.
  - Composition limitation: the production path still needs a real environment, an OS keyring entry, and a reachable provider, so its live success remains unproven and is not claimed; only the deterministic seams are proven.
  - Rollback: remove the `runtime` module and its tests, revert the `runner` boundary and its tests, the `lib.rs`/`main.rs` doc updates, the `destiny-app` credential re-exports, the `destiny-cli` manifest dependency, and the README runtime section.
  - Independent verification: focused composition verification passed with no blocker/high finding; its only medium was the stale evidence line corrected here.
  - Evidence: commit `318c3db` (`feat: compose production CLI reads`). Shared runtime foundations were selectively cherry-picked as `0208774` from upstream `7dc4984`; the sole `destiny-app/src/lib.rs` conflict was resolved additively without losing `QueryOperations` or presentation-safe exports.

- [x] **DCR-005 — Verify and close WU-008**
  - Status: complete.
  - Outcome: independently verified the complete committed `2b6c001..318c3db` range against WU-008 and every tracker acceptance criterion. The production binary now composes the shared read runtime and dispatches valid requests instead of returning the former unsupported fallback. No blocker/high/medium finding remains.
  - Checks: `cargo fmt --all -- --check`; workspace all-target/all-feature check; workspace default and all-feature all-target clippy with `-D warnings`; workspace locked tests default and all features; focused query journey 14 passed; CLI 54 passed; CLI/MCP help; `git diff --check`; clean status. Final independent verification repeated CLI 54, query journey 14, help, clean status/diff, and sanitized human/JSON configuration-error runtime exercises.
  - Runtime harness: the deterministic real SQLite/gateway/query CLI journey proves list/search/inspect, fresh reuse, stale refresh, ambiguity, missing, typed failures, bounds, and secret absence. Scripted production foundations prove acquisition ordering and error mapping. Live provider, OS-keyring round trip, and system-browser/reauthorization UX remain explicit manual limitations and are not claimed.
  - Review workload: six implementation/evidence commits plus this closure commit, including the selectively integrated shared runtime-foundation commit. Large parser/render/test slices retain behavior and tests together and remain independently revertible in reverse order.
  - Rollback: reset this feature branch to `2b6c00159ebc574d1e97d9ffc490244d04f1ca70` to remove only WU-008 plus its selectively integrated runtime foundation.
  - Evidence: pending closure commit.

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
