# Ghostlink exit codes

With `--json`, success prints one object on stdout and a non-zero exit prints
one object on stderr. (`ghostlink login` also prints an `authorizing`
instruction object on stdout before waiting.) The failure object is either:

- `{"error":{"code":"...","message":"..."}}` for every command, including
  `equip` argument, configuration and authorization failures; or
- an equip result with `status` and `reason`, once `equip` has validated its
  arguments and is selecting the character and item.

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
| 30 | `stale_state` | State changed underneath the operation. Run `ghostlink sync`, re-check, and ask before any new equip. |
| 31 | `ambiguous_match` | Show candidates and ask the user which one. |
| 32 | `item_not_found` | No such instance ID; search again with `ghostlink search-inventory`. |
| 33 | `definitions_unavailable` | Run `ghostlink sync-manifest`, then repeat once. |
| 40 | `unsupported_operation` | Report; do not work around it. |
| 41 | `partial_failure` | Report what completed; do not retry. |
| 42 | `verification_failed` | Run `ghostlink sync`, inspect the item, report. |
| 50–52 | `credential_store_unavailable` / `local_storage` / `configuration_error` | Local setup problem; report to the user. |
| 70 | `internal_error` | Report and stop. |

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
| 43 | `lifecycle_busy` | Wait for the other lifecycle command or inspect unsafe lock files manually; never automatically delete foreign locks. This is distinct from equip's pending-provider-refresh exit 43. |
| 51 | `local_storage` | Check installation ownership and permissions; no automatic elevation. |

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
