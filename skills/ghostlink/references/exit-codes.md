# Ghostlink exit codes

With `--json`, success prints one object on stdout and a non-zero exit prints
one object on stderr. (`ghostlink login` also prints an `authorizing`
instruction object on stdout before waiting.) The failure object is either:

- `{"error":{"code":"...","message":"..."}}` for every command, including
  `move` / `equip` argument, configuration and authorization failures; or
- a move or equip result with `status` and `reason`, after argument validation.

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
| 30 | `stale_state` | State changed underneath the operation. Run `ghostlink sync`, re-check, and ask before any new move or equip. |
| 31 | `ambiguous_match` | Show candidates and ask the user which one. |
| 32 | `item_not_found` | No such instance ID; search again with `ghostlink search-inventory`. |
| 33 | `definitions_unavailable` | Run `ghostlink sync-manifest`; repeat reads once. For mutations, re-check state and explicit intent before another invocation. |
| 40 | `unsupported_operation` | Report; do not work around it. |
| 41 | `partial_failure` | Report what completed; do not retry. |
| 42 | `verification_failed` | Run `ghostlink sync`, inspect the item, report. |
| 50–52 | `credential_store_unavailable` / `local_storage` / `configuration_error` | Local setup problem; report to the user. |
| 70 | `internal_error` | Report and stop. |

## Move `status`

| `status` | Exit | Agent action |
|---|---:|---|
| `verified_success` | 0 | Freshly observed at the destination; report it. |
| `verified_noop` | 0 | Reached the destination between planning and fresh preflight; no transfer dispatched. |
| `refused` | varies | Report `reason` and steps; do not retry. Already in the destination when planning is exit **40** (`unsupported_operation`), not no-op success. Missing item is 32, missing character 2, ambiguity 31, changed preflight 30. |
| `incomplete` | 41 | Report completed, rejected/uncertain and unrun steps; no rollback. Do not retry. |
| `verification_failed` | 42 | Run `ghostlink sync`, inspect the item, report uncertainty. |
| `applied_pending_provider_refresh` | 43 | Provider accepted every step but refreshed profile lags. Not verified; check later, never repeat blindly. |

Move may be rejected by Bungie for an equipped item or full destination after
dispatch. A character-to-character route uses the Vault and may stop there.

## Equip `status`

| `status` | Exit | Agent action |
|---|---:|---|
| `verified_success` / `verified_noop` | 0 | Item is equipped on the target. Report it. |
| `refused` | varies | Report `reason` and `steps`; a refusal during execution can follow a completed transfer, so never assume nothing changed. Do not retry. Exit 32 means the item selector matched nothing, 2 means the character matched nothing, other exits follow the error table. |
| `incomplete` | 41 | Some steps ran. Report each step; do not retry. |
| `verification_failed` | 42 | Outcome uncertain. Run `ghostlink sync`, inspect the item, report. |
| `applied_pending_provider_refresh` | 43 | Bungie accepted it but the profile has not caught up. Not verified; suggest checking later. |

Equip requires the character to be in orbit, a social space, or offline;
Bungie may refuse after a transfer already happened.
