# Ignore stray requests on the loopback OAuth callback

## Authorization and scope

User chose option A (2026-10-05): harden the product receiver instead of only
patching the flaky test. Scope: `LoopbackCallbackReceiver` in
`crates/destiny-adapters/src/oauth.rs` and its tests. No change to the manual
paste, headless forwarding or rendezvous receivers.

## Root cause

`oauth::tests::callback_receiver_times_out_without_a_connection` failed ~1%
(3/200 suite, 3/400 isolated) with `UnsupportedOperation` instead of
`CallbackTimeout`. Instrumentation showed `GET / HTTP/1.1` from
`Go-http-client/1.1`; a non-responding trap listener plus `ss -tnpH` attributed
every probe to the local `moshi-hook` process, which probes newly opened
127.0.0.1 listening ports. Any such request during `ghostlink login` aborts the
login before the real callback arrives.

## Design

- A well-formed request whose path is not the callback path is foreign: respond
  `404 Not Found`, close it, and keep accepting until the same accept deadline.
- Unchanged and still fatal: anything addressed to the callback path that is
  invalid (method, state, conflicting outcomes), malformed requests, oversized
  requests and incomplete reads.
- The single accept deadline is shared across foreign connections, so stray
  traffic cannot extend the login window.

## Tasks

- [x] CS-001 RED: tests for a foreign request followed by the real callback and
  for foreign-only traffic ending in `CallbackTimeout`. Observed RED: both new
  tests failed on the missing `404 Not Found` response.
- [x] CS-002 GREEN: classify foreign requests in `parse_request` and keep
  accepting in `receive`. `parse_request` returns `Ok(None)` for a well-formed
  request to another path (method checked only for the callback path);
  `receive` loops over `serve_next` with one shared deadline. Parser tests
  updated for the `Option` shape. `oauth::` 43 passed.
- [x] CS-003 Verify workspace (fmt, clippy, tests, isolated stress) and commit.
  First verify (`muvad4bw-1-scil`) FAIL: fmt/clippy pass, workspace 1280/0,
  all-features 1295/0, adapters 508/0; two static defects fixed with RED
  first: (1) deadline checked only after a successful accept, so queued
  connections were served past it (now checked before every accept);
  (2) an empty or non-token method was classified foreign (method must now be
  non-empty ASCII uppercase before path classification). `oauth::` 44 passed.
  Re-verify (`muvan8cl-2-wsfk`) PASS: fmt, clippy `-D warnings`, workspace
  1281/0, all-features 1296/0, `oauth::tests::` stress 0/200 failed runs
  (8800 tests), no new defect in the diff review. Committed on
  `fix/oauth-callback-stray-requests`.

## Follow-ups

- `callback_receiver_enforces_one_deadline_across_slow_drip_reads` failed 1/200
  in stress (`200 OK` without the failure body). Hypothesis, pre-existing and
  unrelated to this change: the client keeps dripping after the receiver
  responds and closes; closing with unread bytes sends RST, which can discard
  the unread response body. Neither `respond` nor that test changed here.

## Evidence
