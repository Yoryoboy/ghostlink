---
name: ghostlink
description: "Trigger: Destiny 2, D2 inventory, vault, weapon perks, armor stats, builds, weekly reset, loadouts, subclass, Postmaster, pull item, move item, equip item, lock item, unlock item. Prefer Ghostlink MCP tools; use the ghostlink CLI safely when needed."
license: MIT OR Apache-2.0
metadata:
  author: "Yoryoboy"
  version: "1.4"
---

## Activation Contract

Use when the user asks about their Destiny 2 account, items, builds, weekly content, or item actions, and Ghostlink MCP tools are available or `ghostlink` is on `PATH`.

Prefer Ghostlink MCP tools available in this session over shelling out to the CLI. Follow their schemas and report structured results; never blindly retry a mutation. The CLI rules below apply to the fallback path.

First-time users should run `ghostlink` in their own terminal for the setup wizard, or `ghostlink login` for sign-in only. One login serves CLI and MCP. Missing Manifest definitions sync automatically after login or on a CLI read; use `ghostlink sync-manifest` for outdated definitions or a missing-catalog MCP error.

## Hard Rules

- For CLI operations, always add `--json`; never scrape human text. Exit 0 prints one JSON object on stdout; a non-zero exit prints one JSON object on stderr. Exceptions: `ghostlink login` first prints an `authorizing` instruction object, and `--help` is plain text.
- Flags are not listed here. Before using a command for the first time in a session, run `ghostlink <cmd> --help`.
- Never guess an item or character. Resolve ambiguity with a read command or ask the user.
- `ghostlink move` and `ghostlink equip` are CLI game mutations. Run them only with explicit user intent for this item and destination. Move requires exactly one explicit character or Vault destination; equip requires a character. Never select defaults.
- `ghostlink lock` and `ghostlink unlock` require explicit user intent for this item and requested state; MCP `set_lock_state` requires the same intent. Already satisfied is a verified no-op. Vault items use one owned character for the provider call, not a destination selection.
- MCP `pull_item` and fallback `ghostlink pull` require explicit user intent for one genuine Postmaster equipment instance ID only. Destination is its freshly observed owner, never an arbitrary character. Names, hashes, stacks and quantity are unsupported; requires login and qualified stored definitions.
- MCP `pull_stack` with `{"item_hash":"7","amount":"3","character":"2"}` (all three canonical decimal strings, no extra fields), or fallback `ghostlink pull-stack <ITEM_HASH> --amount <N> --character <CHARACTER_ID>`, requires explicit hash, amount and Postmaster owner intent. Hash is canonical uint32 decimal; amount positive int32; owner positive int64, never a class. Only qualified non-instanced account-wide Consumables/Modifications are supported; do not synthesize stack IDs or select destinations. Requires login and stored definitions. Duplicate rows constrain amount by the smallest matching stack, never their sum.
- Never blindly retry `pull-stack`, `pull`, `move`, `equip`, `lock` or `unlock`, including after a non-zero exit. Re-read state (`ghostlink sync`, then `ghostlink inspect-item`) and report. For pull, exit 43 means arrival not yet observed; application is not confirmed. For move, equip, lock and unlock, exit 43 means applied but pending provider refresh. In either case, check later, do not repeat the mutation.
- Move already at its destination during planning is refused (exit 40), not a verified no-op. Report it without working around the refusal.
- `ghostlink update` and `ghostlink uninstall` are installation mutations, not game actions or reads. Run them only with explicit user intent; never automatically retry a failed or uncertain lifecycle command.
- Never try to free inventory space, unequip elsewhere, or change plugs; the CLI refuses these by design.
- Run `ghostlink login` only when the user asks or after exit 10/11; it needs a human to authorize in a browser and waits for the callback.
- `ghostlink handler install` and `ghostlink handler uninstall` change the user's desktop configuration; run them only when the user asks. `ghostlink handler status` is read-only.
- Explain perk, mod, aspect, fragment and ability effects only from output `description` (official Bungie tooltip) and optional `clarity` (community research). Attribute community numbers to Clarity; never invent numbers, percentages or durations absent from both fields. If neither gives an exact value, say it is unavailable.
- Missing `clarity` is not evidence of no effect. For `clarity_match: "variant"`, explicitly qualify the text as an enhanced/base fallback, not exact values for the inspected variant; never infer improved numbers.

## Decision Gates

| Intent | Command |
|---|---|
| Which characters exist, their IDs and light | `ghostlink list-characters` |
| Find owned items by name, type, perk, mod, stat, slot | `ghostlink search-inventory` |
| Full detail of one owned item, including what each perk and mod does | `ghostlink inspect-item` (the `instance` ID from search hits, never the definition `hash`) |
| Committed Postmaster instances and anonymous stacks (read only) | MCP `get_postmaster`; fallback `ghostlink postmaster` |
| Recover one genuine Postmaster equipment instance to its owner | MCP `pull_item`; fallback `ghostlink pull` |
| Recover an explicit amount of a non-instanced stack from one Postmaster owner | MCP `pull_stack`; fallback `ghostlink pull-stack` |
| Current subclass setup and what each equipped plug does | `ghostlink subclass` |
| Unlockable supers, aspects, fragments | `ghostlink subclass-options` |
| Artifact points and active perks | `ghostlink artifact` |
| Saved in-game loadouts | `ghostlink loadouts` |
| Weekly rotation predictions (login required) | `ghostlink weekly-reset` |
| Weekly progress for one character | `ghostlink weekly` |
| Data looks outdated or after an in-game change | `ghostlink sync` |
| Item names or descriptions missing, or definitions outdated | `ghostlink sync-manifest` |
| Move one owned item to a character or Vault | `ghostlink move` |
| Equip an item on a character | `ghostlink equip` |
| Protect one owned item from dismantling | `ghostlink lock` |
| Remove one owned item's lock | `ghostlink unlock` |
| Authorize this machine | `ghostlink login` |
| Explicitly update the Ghostlink installation | `ghostlink update` |
| Explicitly remove the Ghostlink installation, preserving user state | `ghostlink uninstall` |
| Browser callback never reaches login (Linux) | `ghostlink handler status` |

`--character` accepts a class name (`hunter`, `titan`, `warlock`) on most commands, but `ghostlink search-inventory --character` requires the exact numeric character ID; get it from `ghostlink list-characters` first.

## Postmaster limits

Prefer MCP `get_postmaster` with `{}` (or `{"character":null}`) for all owned
characters, or `{"character":"hunter"}` for an exact ID or unique case-insensitive
class. Only optional `character` is accepted: strings must be 1–512 characters,
nonblank and control-free; unknown fields and wrong types are refused before reads.
CLI fallback: `ghostlink postmaster --json`, or
`ghostlink postmaster --character hunter --json`. Ordinary freshness sync applies;
refresh errors do not serve stale fallback. All-character reads include empty groups.
Separate `instances` and `stacks` preserve duplicate observations. Only instances
have real string `instance_id` values; never synthesize IDs for anonymous stacks or
use a stack row as a stable pull selector. Stack `bucket_hash` is the actual runtime
bucket; instances have no inferred bucket. Nullable names fall back to hashes.
`profile_currencies_coverage` qualifies component 103 only: `available` does not
mean complete Postmaster coverage. Report committed observations, not guaranteed
live holdings. Neither read surface pulls items.

For explicit recovery intent, prefer MCP `pull_item` with
`{"item":"700000000000000001"}`; fallback:
`ghostlink pull 700000000000000001 --json`. Use the real `instance_id` from
the instances array, never a hash or stack row. MCP accepts only this required
string field; no destination, character, membership, quantity or other fields.
The ID must be canonical positive int64, without leading zeros. Fresh planning
and preflight qualify non-destructive equipment and pinned target-bucket capacity
against a freshly fetched public Manifest index; unknown or changed evidence
refuses. One action maximum, at most three post-action observations, two seconds
between attempts, no redispatch or rollback. Only `disposition: "verified"`
succeeds. Report receipt separately from arrival, and retain every observation's
classification AND failure. Acknowledgement alone is not application proof;
arrival with a timestamp failure is not success. Refusal stays refused even if
arrival is observed. Exit 43 for pull means not yet observed, not confirmed
application. Caller-local serialization does not cover other processes or game
activity, and observation is not causal or atomic proof. Anonymous stacks use
MCP `pull_stack` with required `item_hash`, `amount`, and `character` decimal
strings (uint32, positive int32, positive int64 respectively), or fallback
`pull-stack`; no names, classes, instance IDs or extra fields. Only qualified
non-instanced account-wide Consumables/Modifications are supported. Duplicate
rows bound amount by the smallest matching stack. Stack arrival requires an exact
amount increase at destination and decrease in Postmaster, with one action and
at most three observations. Live checks remain pending. MCP uses the same shared JSON and
marks every disposition except `verified` with `isError=true`. `not_yet_observed`
is not confirmed application. Never retry pending, uncertain or refused outcomes
blindly; re-read later without dispatching another action.

## Installation Update

- Supported only on Linux x86_64 GNU, using the validated official sibling updater. Unsupported installations refuse mutation; use the original installation method rather than work around validation.
- Windows returns `unsupported_operation` without launching or deferring anything. Tell the user to close Ghostlink, stop `destiny-mcp`, and run the official sibling `.\ghostlink-update.exe` from its installation directory in a separate PowerShell session.
- Update preserves configuration, data and credentials. Restart MCP hosts afterward to load `destiny-mcp`.
- `status: updater_completed` means only the updater completed, not that a requested version was installed; `installed_version` is null. No atomic paired update or rollback is guaranteed. After failure or an uncertain outcome, inspect the installation first; never automatically retry.

## Installation Uninstall

- Preview with `ghostlink uninstall --dry-run --json`; it is strictly read-only, including no lock creation. Mutation in JSON requires `ghostlink uninstall --yes --json`, only after explicit user authorization. No purge is supported.
- Linux x86_64 GNU official receipt-owned installations only. Missing updater is optional; source/manual/unknown installations refuse. Windows refuses before lookup/spawn: ask the user to stop MCP hosts and active login, close Ghostlink, then inspect the receipt and remove only owned files/integrations from a separate PowerShell session (or use the original package manager).
- Preserve all configuration, databases, credentials, runtime state, external/custom paths, unknown files/backups, Cargo directories and PATH/shell config. Config is never loaded to resolve overrides. Stop MCP hosts and active login first; no process inactivity guarantee or termination.
- Exact bundled skill identity is not installation provenance. Modified/older skills remain. Only exact installation-owned OpenCode commands and proven URI entries are eligible; name-only Claude/Codex/Pi registrations always require manual followup. Unsafe handler cleanup retains binaries/receipt.
- Exit 0 can mean preview, cancelled, or removed_with_manual_followups, never unconditional removal of all integrations. Exit 41 partial_failure is one stderr JSON object containing both error and report, including completed/remaining per-path outcomes. Individual changes are not transactional; no rollback. Lock metadata remains intentionally. Report manual followups and preserved locations; never blindly retry.

## Descriptions

- Five plug surfaces expose optional official `description` and community `clarity`: subclass slots, subclass options, artifact slots, loadout plugs and inspect `plug_details` (`hash`, `name`, `role`, optional `description`, `clarity`, `clarity_match`). Missing fields mean no text is available, not no effect.
- `ghostlink search-inventory` returns names only. To explain perks, search first, then inspect the chosen instance.
- Descriptions exist only for plugs present in the user's items, subclass or subclass options; there is no lookup for arbitrary perk names.

## Weekly Calendar

- Calendar values are predictions from a community-derived rotation, never confirmed rotations.
- For the current week, prefer observed live milestones when they disagree with the calendar, and mention the disagreement.
- For "what is featured next week", use MCP `date` (`YYYY-MM-DD`, UTC end of day) or CLI `--date` (local end of day); check the returned window.
- "No data" means outside verified coverage; do not guess.

## Execution Steps

1. Pick the command from the table; check its `--help` if not yet seen.
2. Run it with `--json`. Reads refresh a stale profile automatically; use `ghostlink sync` only to force freshness.
3. On exit 0, answer from the JSON. Mention `metadata.checked_at` when freshness matters.
4. On non-zero exit, parse stderr. An object with `status` is a move, equip, lock or unlock result: read `status` and `reason`. For `kind: "instanced_pull"` or `kind: "stack_pull"`, read `disposition`, independent `receipt`, `failure` and all ordered `observations`. Otherwise read `error.code`. Then follow `references/exit-codes.md`.
5. For mutations, quote the resolved item and destination (move/equip) or requested lock state (lock/unlock) back to the user if inferred from earlier conversation; obtain explicit intent before acting. For pull, confirm the exact genuine instance ID and its observed Postmaster owner.

## Output Contract

- State facts only from command output; say when a result is empty instead of inventing items.
- For move or equip, report `status`, each step, and `reason`; never claim rollback after partial progress. Move verifies location, equip verifies location and equipped state. Exit 43 is applied but not yet visible: tell the user to check again later; it is not verified success. Update exit 43 instead means `lifecycle_busy`.
- For lock/unlock, report `status`, requested `locked`, `attempt`, `observed` and `reason`. Only verified success/no-op proves the lock state. Exit 41/42/43 is not verified success; never blindly retry.
- For pull, report `disposition`, `receipt`, `failure` and all ordered observation classifications and failures. Only verified arrival is success; pending, uncertain and refused are nonzero. Never interpret a receipt as proof or blindly repeat the action.
- For `pull-stack`, report receipt separately from disposition and ordered observations, including classification AND failure and pre/post destination, whole-profile and owner Postmaster quantities. Only `verified` succeeds; lag remains `uncertain`/42, not confirmed application. Null is unknown, not zero. Re-read with sync/postmaster later; never blindly repeat. Confirm exact hash, amount and owner before acting. Observations are not causal or atomic guarantees.
- Never echo tokens, membership IDs, or raw provider errors.

## References

- `references/exit-codes.md` — error codes, exits, and the agent action for each.
