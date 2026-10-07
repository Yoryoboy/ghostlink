---
name: ghostlink
description: "Trigger: Destiny 2, D2 inventory, vault, weapon perks, armor stats, builds, weekly reset, loadouts, subclass, equip item. Prefer Ghostlink MCP tools; use the ghostlink CLI safely when needed."
license: MIT OR Apache-2.0
metadata:
  author: "Yoryoboy"
  version: "1.2"
---

## Activation Contract

Use when the user asks about their Destiny 2 account, items, builds, weekly content, or item actions, and Ghostlink MCP tools are available or `ghostlink` is on `PATH`.

Prefer Ghostlink MCP tools available in this session over shelling out to the CLI. Follow their schemas and report structured results; never blindly retry a mutation. The CLI rules below apply to the fallback path.

First-time users should run `ghostlink` in their own terminal for the setup wizard, or `ghostlink login` for sign-in only. One login serves CLI and MCP. Missing Manifest definitions sync automatically after login or on a CLI read; use `ghostlink sync-manifest` for outdated definitions or a missing-catalog MCP error.

## Hard Rules

- For CLI operations, always add `--json`; never scrape human text. Exit 0 prints one JSON object on stdout; a non-zero exit prints one JSON object on stderr. Exceptions: `ghostlink login` first prints an `authorizing` instruction object, and `--help` is plain text.
- Flags are not listed here. Before using a command for the first time in a session, run `ghostlink <cmd> --help`.
- Never guess an item or character. Resolve ambiguity with a read command or ask the user.
- `ghostlink equip` is the only CLI game mutation. Run it only when the user explicitly named both the item and the character for this action.
- `ghostlink update` is an installation mutation, not a game action or read. Run it only with explicit user intent; never automatically retry a failed or uncertain lifecycle command.
- Never retry `equip` after a non-zero exit. Re-read state (`ghostlink sync`, then `ghostlink inspect-item`) and report.
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
| Current subclass setup and what each equipped plug does | `ghostlink subclass` |
| Unlockable supers, aspects, fragments | `ghostlink subclass-options` |
| Artifact points and active perks | `ghostlink artifact` |
| Saved in-game loadouts | `ghostlink loadouts` |
| Weekly rotation predictions (login required) | `ghostlink weekly-reset` |
| Weekly progress for one character | `ghostlink weekly` |
| Data looks outdated or after an in-game change | `ghostlink sync` |
| Item names or descriptions missing, or definitions outdated | `ghostlink sync-manifest` |
| Equip an item on a character | `ghostlink equip` |
| Authorize this machine | `ghostlink login` |
| Explicitly update the Ghostlink installation | `ghostlink update` |
| Browser callback never reaches login (Linux) | `ghostlink handler status` |

`--character` accepts a class name (`hunter`, `titan`, `warlock`) on most commands, but `ghostlink search-inventory --character` requires the exact numeric character ID; get it from `ghostlink list-characters` first.

## Installation Update

- Supported only on Linux x86_64 GNU, using the validated official sibling updater. Unsupported installations refuse mutation; use the original installation method rather than work around validation.
- Windows returns `unsupported_operation` without launching or deferring anything. Tell the user to close Ghostlink, stop `destiny-mcp`, and run the official sibling `.\ghostlink-update.exe` from its installation directory in a separate PowerShell session.
- Update preserves configuration, data and credentials. Restart MCP hosts afterward to load `destiny-mcp`.
- `status: updater_completed` means only the updater completed, not that a requested version was installed; `installed_version` is null. No atomic paired update or rollback is guaranteed. After failure or an uncertain outcome, inspect the installation first; never automatically retry.

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
4. On non-zero exit, parse stderr. An object with `status` is an equip result: read `status` and `reason`. Otherwise read `error.code`. Then follow `references/exit-codes.md`.
5. For equip, quote the resolved item and character back to the user before running if either was inferred from earlier conversation.

## Output Contract

- State facts only from command output; say when a result is empty instead of inventing items.
- For equip, report `status`, each step, and `reason`. Equip exit 43 is applied but not yet visible: tell the user to check again later; it is not verified success. Update exit 43 instead means `lifecycle_busy`.
- Never echo tokens, membership IDs, or raw provider errors.

## References

- `references/exit-codes.md` — error codes, exits, and the agent action for each.
