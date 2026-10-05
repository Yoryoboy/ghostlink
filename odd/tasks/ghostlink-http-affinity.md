# Shared Bungie HTTP affinity

> **Status: DROPPED before merge (2026-10-05).** Live evidence below showed the
> post-mutation contradiction is provider-side staleness, not lost client
> affinity; it is handled by `ghostlink-pending-provider-refresh.md`. The shared
> session, cookie dependencies and wiring were reverted and never reached main
> (unused dependency closure, Rust 1.88 requirement and maintenance cost with no
> observed benefit). Commit refs below exist only on the local backup branch
> `feature/equip-item`. Kept as the investigation record.

## Authorization and scope

User approved shared HTTP affinity correction and Cargo registry downloads with
manifest/lock updates. Preserve existing dirty equip/sync work on feature/equip-item.
No commits, push, PR, game mutations or live account reads in this feature.
Historical Ghost Primus contradiction is consistent with affinity loss but its
cause remains unproven; do not rewrite historical verification FAIL.

## Design and safety

Use one account-session-scoped blocking reqwest client and standard cookie jar,
shared by profile/transfer/equip in CLI and MCP. In-memory only; opaque Debug;
exact origin https://www.bungie.net:443; standard domain/path/secure/expiry rules;
bounded cookie input/storage. No cross-account/global jar, arbitrary public client
injection or guessed cookie names. OAuth/manifest remain separate.
Preserve allowlists before credentials, TLS verification, redirects/proxies off,
15s total/5s connect timeouts, response limits and one-shot mutation dispatch.
No sleeps, mutation retries, polling or origin-metadata expansion.

## Tasks

- [x] HA-001 Enable standard cookie support and acquire dependency closure.
  reqwest cookies enabled; 12 new package blocks / 2 reqwest dependency edges.
  Cargo fetch, offline locked metadata and adapters check PASS; diff clean.
  In-memory baseline reconstruction recovers prior lock SHA exactly: no prior
  versions/checksums/edges changed. New closure requires Rust 1.88; current
  workspace declares no rust-version. No meaningful runtime RED for dependencies.
  Commit `08db447`.
- [x] HA-002 Implement hardened shared session and real-wire transport tests.
  BungieHttpSession (new/Clone/opaque Debug) with bounded cookie_store jar,
  exact-origin capture/replay, retries off, HTTPS enforced. RED: `cargo test
  --locked -p destiny-adapters http_session` exit 101 "shared affinity absent on
  real TLS request" (Domain=bungie.net wire case); GREEN: 7/7 (8/8 after
  HA-004 fixes) tests over private
  loopback TLS (replay profile->transfer->equip, account/origin isolation,
  path/domain/expiry, capacity replacement/deletion, bounds fail-closed +
  recovery, redirects, cert trust). Clippy all-targets PASS; rustfmt was only
  checked on session files (workspace fmt failed, fixed in HA-004).
  Commit `34c46b5` (includes HA-004 adapter fixes).
- [x] HA-003 Wire CLI and MCP production composition and regressions.
  CLI compose_equip and MCP compose_server each allocate one local
  BungieHttpSession shared by profile/transfer/equip via with_session;
  ReadFoundations unchanged (standalone profile gets its own session);
  OAuth/manifest and credential-source clones untouched. Initial source-text
  regressions were replaced in HA-004 by runtime tests.
  RED: MCP regression 0 session allocations vs expected 1; GREEN: full
  destiny-mcp suite PASS. adapters http_session 7/7, clippy cli+mcp, rustfmt
  PASS. `cargo test -p destiny-cli` FAIL before and after (inherited
  destiny_app::test_support gate), so CLI regression unexecuted standalone.
  Commit `792b255` (includes HA-004 runtime regressions).
- [x] HA-004 Independent functional/security review and full offline checks.
  Verify: workspace clippy (default/all-features), workspace tests
  (default/all-features, adapters 412/412), git diff --check PASS; fmt FAIL in
  profile_gateway.rs/transfer.rs. Judge A (security): no findings. Judge B: one
  rejected Set-Cookie wiped the whole jar (fixed: ignore offending cookie,
  clear only on byte/count bounds); path test asserted absence only (fixed:
  scoped replay asserted on transfer, absent on profile); source-text
  regressions replaced by runtime proof (BungieHttpSession::shares_state_with
  via Arc::ptr_eq, doc-hidden session accessors, private transport factories
  consumed by compose_equip/compose_server; tests
  equip_composition_shares_only_invocation_local_affinity and
  server_composition_shares_only_account_local_affinity assert sharing and
  isolation); fmt fixed. RED->GREEN observed for cookie fixes (8/8); factory
  RED was compile-level only. Re-judgment: only leftover empty
  tests/http_affinity.rs files, deleted. Post-fix: both runtime tests ok,
  cargo fmt --all --check PASS, workspace test + all-features clippy PASS
  (writer-run). Fixes folded into `34c46b5` and `792b255`.

## Evidence

Read-only maps muud51m9-n-9fy6 and muudibfe-o-anij: separate reqwest clients
ignore affinity response data. Official Bungie transfer FAQ documents obsolete
server copies when response-established affinity is discarded. reqwest 0.13.5
cookies requires cookie ^0.18 and cookie_store ^0.22; latter/publicsuffix absent
from offline cache before authorized fetch. Standard jar must be guarded, not
replaced by a homemade parser.

## Checks and delivery

Use persistent target/. After dependency acquisition, CARGO_NET_OFFLINE=true
locked focused tests, fmt, clippy, workspace/default and all-feature tests,
diff check. Known inherited standalone CLI test_support gate remains separate.
Record exact outcomes and pending limitations per transition. No commits under
current user constraint; remote delivery remains unperformed.

## Commits

User authorized commits on 2026-10-04 (no push/PR). Work split by feature:
each commit's isolated tree passed fmt, all-features clippy/check and
workspace all-features tests; final tree: default 1281 / all-features 1295
passed, clippy 0 warnings, fmt and diff check PASS; HEAD byte-identical to the
pre-split backup.

## Live read-only smoke (authorized 2026-10-04)

On `be800e3`, `ghostlink --json sync` exit 0 through the shared session:
fetched, committed, revision 8 (previous 8, unchanged), checked_at 1791157321.
`inspect-item 6917529045285107589` exit 0: Ghost Primus on Hunter
`2305843009300568190`, equipped=true, fresh. Proves the cookie-enabled
profile transport works live; it does NOT exercise mutation->refresh affinity,
so it neither proves nor disproves the historical contradiction cause. That
needs a separately authorized equip mutation.

## Live equip smoke (user authorized "any item", 2026-10-04)

Atlas Shell (Ghost) refused pre-dispatch, exit 40 ("no supported weapon or
armor equipment slot"), no steps. Antiope-D `6917529047567222995` Vault ->
Hunter `2305843009300568190`, one call, exit 42 `verification_failed`:
transfer completed, equip completed, refresh refreshed but observed Vault /
equipped=false. Follow-up forced sync exit 0 at checked_at 1791158683:
revision 8 unchanged (identical profile content to before the mutation),
Antiope-D Vault/false, Ghost Primus still Hunter/equipped=true. No retry.
Conclusion: contradiction REPRODUCES with shared cookie affinity, so
client-side affinity loss is not the (sole) cause. Leading hypothesis now is
provider-side stale/cached GetProfile responses after mutations (earlier
incident converged on a later sync). Unproven; whether Bungie set any cookie
was not observed. Shared session remains harmless hardening.

Delayed forced sync (user-requested, checked_at 1791159624, ~16 min after the
mutation): exit 0, revision 9 (previous 8, changed). Antiope-D on Hunter,
equipped=true; Ghost Primus on Hunter, equipped=false. The mutation applied;
the immediate and +~20s reads were stale provider data that later converged.
Convergence time is bounded only above (<= ~16 min); not measured.
