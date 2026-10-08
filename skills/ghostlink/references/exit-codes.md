# Ghostlink exit codes

With `--json`, success prints one object on stdout and a non-zero exit prints
one object on stderr. (`ghostlink login` also prints an `authorizing`
instruction object on stdout before waiting.) The failure object is either:

- `{"error":{"code":"...","message":"..."}}` for every command, including
  `pull-stack` / `pull` / `move` / `equip` / `lock` / `unlock` argument, configuration and authorization failures; or
- a mutation result with `status` and `reason`, after argument validation; or
- `kind: "instanced_pull"` or `kind: "stack_pull"` with independent
  `disposition`, `receipt` and `failure` for pull planning/execution evidence.

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

## Apply loadout `outcome`

`ghostlink apply-loadout <slot|name> --character <class|id> --json` and MCP
`apply_loadout` share `kind: "apply_loadout"`, character, one-based slot, name,
outcome, receipt and reason. Exactly one MCP slot/name; names are normalized exact
matches within that character. Unknown/ambiguous names list candidates; select
explicitly, never guess. Empty/unavailable slots refuse without an action.

| `outcome` | Exit | Agent action |
|---|---:|---|
| `applied` | 0 | Acknowledged, not observed. Next ordinary read forces sync. |
| `refused` / `failed` | 41 | Report reason/receipt/candidates; no automatic retry. Location refusal: go to orbit or a social space and retry only with reassessed intent. |
| `local_state_failed` | 42 | Local state could not be updated after the action; receipt retained. Sync/read, never blindly repeat. |

Startup/argument errors use the usual error envelope and exits. Requires login,
Manifest definitions and orbit/social space/offline. No post-action request,
provider-lag exit 43 or retry; every dispatched attempt invalidates freshness.

## Pull `disposition` (CLI genuine-instance equipment only)

| `disposition` | Exit | Agent action |
|---|---:|---|
| `applied` | 0 | Bungie acknowledged the action, not observed arrival. Next ordinary read forces sync. |
| `uncertain` | 41 | No clear acknowledgement; sync/inspect and reassess intent, never blindly retry. |
| `refused` | 41 / varies | Received refusal exits 41; pre-action refusal retains category exits. Report without working around it. |
| `local_state_failed` | 42 | Local state could not be updated after the action; receipt retained. Sync/inspect, do not retry. |

Planning missing target is 32, ambiguity 31, missing definition 33; preflight
drift/expiry is 30, other eligibility refusals default to 40. Typed pre-action
infrastructure errors use the category table above. No post-action profile request,
wait or observations; no provider-lag exit 43. Dispatched receipts invalidate local
freshness durably, without guessed state. Pre-action failures have no receipt.
All nonzero results stay on stderr as complete structured evidence.

Pull accepts only a canonical positive int64 instance ID, not names, hashes,
quantity, destination overrides or anonymous stacks. Requires authenticated
membership and freshly eligible original Postmaster ownership, stored definitions
and fresh pinned Manifest capacity qualification. No full-stack/live-readiness
claim. MCP `pull_item` uses the same shared output.

## Stack pull `disposition`

`ghostlink pull-stack <ITEM_HASH> --amount <N> --character <CHARACTER_ID>`
uses `kind: "stack_pull"` with independent receipt, disposition and failure.
Canonical uint32 hash, positive int32 amount and positive int64 owner ID are
required; invalid input exits 2 before any I/O. The pull disposition table above
also applies: `applied` is 0, received refusal/transport uncertainty 41, local
invalidation storage failure 42. No provider-lag exit 43, post-action request,
wait or observations; preflight totals are not invented post quantities.
Missing definition is 33, ambiguous definition 31, preflight drift/expiry 30;
other pre-action failures retain category exits. Nonzero evidence stays on stderr.
Requires authenticated membership, qualified non-instanced metadata and fresh
pinned account-wide capacity evidence. Owner selects the Postmaster source, not
a destination. Sync/read Postmaster and reassess explicit hash/amount/owner intent
before another action. Acknowledgement is not an atomic or observed-state guarantee;
offline checks do not establish live readiness.

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
Handler skips add `expected_status: "partial"` and `warnings` to JSON/human previews
(exit 0). Follow the concrete MIME/desktop/record paths in `manual_followups`:
select another installed desktop ID with
`xdg-mime default <other>.desktop x-scheme-handler/ghostlink`, or remove only
Ghostlink's scheme associations from mimeapps.list. Verify Ghostlink is no longer
selected and verify ownership before deleting the listed desktop entry/record.

| Exit | Code / status | Agent action for uninstall |
|---:|---|---|
| 0 | preview / cancelled / removed_with_manual_followups | Report per-path outcomes, preserved locations and manual followups. |
| 2 | invalid_request / confirmation_required | Check help; preview first, use `--yes` only with explicit user authorization. |
| 40 | unsupported_installation / unsupported_operation | Use the original installation method. Windows: close Ghostlink, stop MCP hosts and active login, inspect the receipt and remove only owned files from separate PowerShell, preserving user data; nothing launches. |
| 41 | partial_failure | One stderr JSON object includes error **and** report; inspect removed/changed paths, agents, handler skips/errors and retained temps. Handler skips preserve handler files but allow owned binaries/updater/receipt removal; `removed` lists completed paths. Handler mutation errors, changed snapshots or agent filesystem failures retain remaining binaries/receipt; final file removals can be individually partial. Do not blindly retry. |
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
| `applied` | 0 | Bungie acknowledged requested lock state, not a post-action observation. Next ordinary read forces sync. |
| `verified_noop` | 0 | Fresh preflight already satisfies requested state; no dispatch. |
| `refused` | varies | Report `reason`; missing item 32, ambiguity 31, stale state 30, otherwise category exit. No dispatch; do not work around refusal. |
| `incomplete` | 41 | Refused, auth-refused, transient or uncertain attempt; no post-action observation. Never blindly retry. |
| `verification_failed` | 42 | Local invalidation storage failed after dispatch: local state could not be updated after the action. Sync and inspect; do not retry. |

Lock/unlock no longer emit provider-lag verification exits 42/43. Dispatched
attempts invalidate freshness, not guessed state; already-satisfied no-ops do not.

Vault items use one owned character for the provider call. Lock/unlock do not
apply equip class, exotic, or equipped-state policy.
