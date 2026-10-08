# Ghostlink exit codes

With `--json`, success prints one object on stdout and a non-zero exit prints
one object on stderr. (`ghostlink login` also prints an `authorizing`
instruction object on stdout before waiting.) The failure object is either:

- `{"error":{"code":"...","message":"..."}}` for every command, including
  `pull-stack` / `pull` / `move` / `equip` / `lock` / `unlock` argument, configuration and authorization failures; or
- a mutation result with `status` and `reason`, after argument validation; or
- `kind: "instanced_pull"` with independent `disposition`, `receipt`, `failure`
  and ordered `observations` for pull planning/execution evidence.

## Error codes

| Exit | `error.code` | Agent action |
|---:|---|---|
| 2 | `invalid_request` | Fix the argument named in the message; check `--help`. |
| 2 | `unknown_stat_name` / `unknown_search_name` | Name not recognized; use a suggestion from the message or ask. |
| 2 | `ambiguous_search_name` | Several perks/mods/archetypes match; pick an exact name or ask. |
| 10 | `authorization_required` / `authorization_denied` / `authorization_state` | Ask the user to run `ghostlink login`. |
| 11 | `reauthorization_required` | Ask the user to run `ghostlink login` again. |
| 20 | `provider_rate_limit` | Wait before any further call; do not loop. |
| 21 | `provider_unavailable` | Bungie unreachable, or a login callback timed out; report and stop. |
| 30 | `stale_state` | State changed underneath the operation. Run `ghostlink sync`, re-check, and ask before any new mutation. |
| 31 | `ambiguous_match` | Show candidates and ask the user which one. |
| 32 | `item_not_found` | No such instance ID; search again with `ghostlink search-inventory`. |
| 33 | `definitions_unavailable` | Run `ghostlink sync-manifest`; repeat reads once. For mutations, re-check state and explicit intent before another invocation. |
| 40 | `unsupported_operation` | Report; do not work around it. |
| 41 | `partial_failure` | Report what completed; do not retry. |
| 42 | `verification_failed` | Run `ghostlink sync`, inspect the item, report. |
| 50–52 | `credential_store_unavailable` / `local_storage` / `configuration_error` | Local setup problem; report to the user. |
| 70 | `internal_error` | Report and stop. |

## Pull `disposition` (CLI genuine-instance equipment only)

| `disposition` | Exit | Agent action |
|---|---:|---|
| `verified` | 0 | Report exact arrival separately from the received receipt; observation is not causal proof. |
| `not_yet_observed` | 43 | Acknowledged but no verified arrival; do not claim application. Check later with sync/inspect, never blindly repeat. |
| `uncertain` | 42 | Report all ordered classifications AND failures, then sync/inspect. Arrival plus timestamp failure remains unverified. |
| `refused` | varies | Report `failure` or received refusal and any arrival separately; do not work around or retry. |

Refusal defaults to 40. Received authentication refusal is 11, transient refusal
21. Planning missing target is 32, ambiguity 31, missing definition 33; preflight
drift/expiry is 30. Typed infrastructure errors use the category table above.
Pre-action failures have no receipt or post-action observations. Confirmed refusal
remains refused even if arrival is observed. All nonzero results stay on stderr
as complete structured evidence, not a generic error replacing that evidence.

Pull accepts only a canonical positive int64 instance ID, not names, hashes,
quantity, destination overrides or anonymous stacks. Requires authenticated
membership and freshly eligible original Postmaster ownership, stored definitions
and fresh pinned Manifest capacity qualification. No full-stack/live-readiness
claim. MCP `pull_item` uses the same shared output.

## Stack pull `disposition`

`ghostlink pull-stack <ITEM_HASH> --amount <N> --character <CHARACTER_ID>`
uses `kind: "stack_pull"` with independent receipt, disposition, failure and
ordered observations. Canonical uint32 hash, positive int32 amount and positive
int64 owner ID are required; invalid input exits 2 before any I/O.
Only `verified` exits 0. Lag and uncertain evidence exit 42; sync and read
Postmaster later, never blindly repeat. The current service does not emit
`not_yet_observed`; its reserved mapping is 43, not confirmed application.
Refusal defaults to 40; received auth 11, transient 21. Missing definition 33,
ambiguous definition 31, preflight drift/expiry 30; infrastructure uses the
existing category table. No new exit codes. Nonzero evidence stays on stderr.
Report all quantity classifications AND failures; null quantity is unknown.
Confirmed refusal remains refused even with arrival. Requires authenticated
membership, qualified non-instanced metadata and fresh pinned account-wide
capacity evidence. Owner selects the Postmaster source, not a destination.
Reassess explicit hash/amount/owner intent after fresh reads. Observed quantities
are neither causal nor atomic proof; offline checks do not establish live readiness.

## Move `status`

| `status` | Exit | Agent action |
|---|---:|---|
| `applied` | 0 | Every step accepted; not an observed destination. Next ordinary read forces sync. |
| `verified_noop` | 0 | Reached the destination between planning and fresh preflight; no transfer dispatched. |
| `refused` | varies | Report `reason` and steps; do not retry. Already in the destination when planning is exit **40** (`unsupported_operation`), not no-op success. Missing item is 32, missing character 2, ambiguity 31, changed preflight 30. |
| `incomplete` | 41 | Report completed, rejected/uncertain and unrun steps; no rollback. Do not retry. |
Move does not emit provider-lag verification exits 42/43. Never blindly repeat.

Move may be rejected by Bungie for an equipped item or full destination after
dispatch. A character-to-character route uses the Vault and may stop there.

## Update outcomes

`ghostlink update` is an installation mutation requiring explicit user intent.
Exit 0 with `status: updater_completed` means the updater completed, not that a
requested version was installed (`installed_version` is null). Configuration,
data and credentials are preserved; restart MCP hosts afterward. Never
automatically retry a failed or uncertain update.

| Exit | `error.code` | Agent action for update |
|---:|---|---|
| 2 | `invalid_request` | Check the command and requested version against `--help`. |
| 40 | `unsupported_installation` / `updater_missing` | Report the refusal; use the original installation method or official release installer, not a substitute updater from PATH. |
| 40 | `unsupported_operation` | Windows refuses without launching or deferring anything. Ask the user to close Ghostlink, stop `destiny-mcp`, then run the official sibling `.\ghostlink-update.exe` from its installation directory in a separate PowerShell session. Other platforms outside Linux x86_64 GNU also refuse. |
| 41 | `updater_failed` | Files may have changed. Inspect the installation first; report and do not automatically retry. No rollback is guaranteed. |
| 43 | `lifecycle_busy` | Wait for the other lifecycle command or inspect unsafe lock files manually; never automatically delete foreign locks. This is unrelated to move/equip acknowledgement outcomes. |
| 51 | `local_storage` | Check installation ownership and permissions; no automatic elevation. |

## Uninstall outcomes

`ghostlink uninstall` requires explicit installation-removal intent. No purge is
supported; configuration, databases, credentials, runtime state, external/custom
paths and unknown files remain. Stop MCP hosts and active login first. No process
inactivity guarantee, elevation or termination. Preview is read-only; JSON mutation
requires explicit `--yes`. Exact skill identity is not installation provenance;
name-only registrations always need manual inspection. Success never guarantees
all integrations are gone. Individual changes have no rollback; lock metadata remains.

| Exit | Code / status | Agent action for uninstall |
|---:|---|---|
| 0 | preview / cancelled / removed_with_manual_followups | Report per-path outcomes, preserved locations and manual followups. |
| 2 | invalid_request / confirmation_required | Check help; preview first, use `--yes` only with explicit user authorization. |
| 40 | unsupported_installation / unsupported_operation | Use the original installation method. Windows: close Ghostlink, stop MCP hosts and active login, inspect the receipt and remove only owned files from separate PowerShell, preserving user data; nothing launches. |
| 41 | partial_failure | One stderr JSON object includes error **and** report; inspect removed/changed paths, agents, handler skips/errors and retained temps. Binaries/receipt stay on unsafe handler or agent filesystem cleanup failure; final file removals can be individually partial. Do not blindly retry. |
| 43 | lifecycle_busy | Wait or inspect unsafe lock metadata manually; do not unlink live/foreign locks. |
| 51 | local_storage | Safe preview/access failed; check permissions, no automatic elevation. |

## Equip `status`

| `status` | Exit | Agent action |
|---|---:|---|
| `applied` | 0 | Every step accepted, not observed equipped state. Next ordinary read forces sync. |
| `verified_noop` | 0 | Fresh preflight already satisfies intent; no dispatch. |
| `refused` | varies | Report `reason` and `steps`; a refusal during execution can follow a completed transfer, so never assume nothing changed. Do not retry. Exit 32 means the item selector matched nothing, 2 means the character matched nothing, other exits follow the error table. |
| `incomplete` | 41 | Some steps ran. Report each step; do not retry. |
Equip does not emit provider-lag verification exits 42/43. Never blindly repeat.

Equip requires the character to be in orbit, a social space, or offline;
Bungie may refuse after a transfer already happened.

## Lock/unlock `status`

| `status` | Exit | Agent action |
|---|---:|---|
| `verified_success` / `verified_noop` | 0 | Requested lock state verified; no-op did not dispatch. |
| `refused` | varies | Report `reason`; missing item 32, ambiguity 31, stale state 30, otherwise category exit. No dispatch; do not work around refusal. |
| `incomplete` | 41 | Report bounded `attempt` and `observed`; never blindly retry. |
| `verification_failed` | 42 | Outcome unverified; sync and inspect, then report. |
| `applied_pending_provider_refresh` | 43 | Provider accepted the action but observed state is stale/unconfirmed; check later, do not repeat. |

Vault items use one owned character for the provider call. Lock/unlock do not
apply equip class, exotic, or equipped-state policy.
