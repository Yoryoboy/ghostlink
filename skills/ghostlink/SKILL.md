---
name: ghostlink
description: "Trigger: Destiny 2, D2 inventory, vault, weapon perks, armor stats, builds, weekly reset, loadouts, subclass, move item, equip item, lock item, unlock item. Prefer Ghostlink MCP tools; use the ghostlink CLI safely when needed."
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
- Never blindly retry `move`, `equip`, `lock` or `unlock`, including after a non-zero exit. Re-read state (`ghostlink sync`, then `ghostlink inspect-item`) and report. Exit 43 means applied but pending provider refresh: check later, do not repeat the mutation.
- Move already at its destination during planning is refused (exit 40), not a verified no-op. Report it without working around the refusal.
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
| Committed Postmaster instances and anonymous stacks (no pull) | `ghostlink postmaster` |
| Current subclass setup and what each equipped plug does | `ghostlink subclass` |
| Unlockable supers, aspects, fragments | `ghostlink subclass-options` |
| Artifact points and active perks | `ghostlink artifact` |
| Saved in-game loadouts | `ghostlink loadouts` |
| Public weekly reset (no account needed) | `ghostlink weekly-reset` |
| Weekly progress for one character | `ghostlink weekly` |
| Data looks outdated or after an in-game change | `ghostlink sync` |
| Item names or descriptions missing, or definitions outdated | `ghostlink sync-manifest` |
| Move one owned item to a character or Vault | `ghostlink move` |
| Equip an item on a character | `ghostlink equip` |
| Protect one owned item from dismantling | `ghostlink lock` |
| Remove one owned item's lock | `ghostlink unlock` |
| Authorize this machine | `ghostlink login` |
| Browser callback never reaches login (Linux) | `ghostlink handler status` |

`--character` accepts a class name (`hunter`, `titan`, `warlock`) on most commands, but `ghostlink search-inventory --character` requires the exact numeric character ID; get it from `ghostlink list-characters` first.

## Postmaster limits

Use `ghostlink postmaster --json`, or `ghostlink postmaster --character hunter --json`
for one exact ID or unique case-insensitive class. Ordinary freshness sync applies;
refresh errors do not serve stale fallback. All-character reads include empty groups.
Separate `instances` and `stacks` preserve duplicate observations. Only instances
have real string `instance_id` values; never synthesize IDs for anonymous stacks or
use a stack row as a stable pull selector. Stack `bucket_hash` is the actual runtime
bucket; instances have no inferred bucket. Nullable names fall back to hashes.
`profile_currencies_coverage` qualifies component 103 only: `available` does not
mean complete Postmaster coverage. Report committed observations, not guaranteed
live holdings. There is no pull feature in this command.

## Descriptions

- Five plug surfaces expose optional official `description` and community `clarity`: subclass slots, subclass options, artifact slots, loadout plugs and inspect `plug_details` (`hash`, `name`, `role`, optional `description`, `clarity`, `clarity_match`). Missing fields mean no text is available, not no effect.
- `ghostlink search-inventory` returns names only. To explain perks, search first, then inspect the chosen instance.
- Descriptions exist only for plugs present in the user's items, subclass or subclass options; there is no lookup for arbitrary perk names.

## Execution Steps

1. Pick the command from the table; check its `--help` if not yet seen.
2. Run it with `--json`. Reads refresh a stale profile automatically; use `ghostlink sync` only to force freshness.
3. On exit 0, answer from the JSON. Mention `metadata.checked_at` when freshness matters.
4. On non-zero exit, parse stderr. An object with `status` is a move, equip, lock or unlock result: read `status` and `reason`. Otherwise read `error.code`. Then follow `references/exit-codes.md`.
5. For mutations, quote the resolved item and destination (move/equip) or requested lock state (lock/unlock) back to the user if inferred from earlier conversation; obtain explicit intent before acting.

## Output Contract

- State facts only from command output; say when a result is empty instead of inventing items.
- For move or equip, report `status`, each step, and `reason`; never claim rollback after partial progress. Move verifies location, equip verifies location and equipped state. Exit 43 is applied but not yet visible: tell the user to check again later; it is not verified success.
- For lock/unlock, report `status`, requested `locked`, `attempt`, `observed` and `reason`. Only verified success/no-op proves the lock state. Exit 41/42/43 is not verified success; never blindly retry.
- Never echo tokens, membership IDs, or raw provider errors.

## References

- `references/exit-codes.md` — error codes, exits, and the agent action for each.
