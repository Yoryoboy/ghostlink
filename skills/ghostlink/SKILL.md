---
name: ghostlink
description: "Trigger: Destiny 2, D2 inventory, vault, weapon perks, armor stats, builds, weekly reset, loadouts, subclass, equip item. Operate the ghostlink CLI safely."
license: MIT OR Apache-2.0
metadata:
  author: "Yoryoboy"
  version: "1.0"
---

## Activation Contract

Use when the user asks about their Destiny 2 account, items, builds, weekly content, or wants an item equipped, and a `ghostlink` executable is on `PATH`.

## Hard Rules

- Always add `--json`; never scrape human text. Exit 0 prints one JSON object on stdout; a non-zero exit prints one JSON object on stderr. Exceptions: `ghostlink login` first prints an `authorizing` instruction object, and `--help` is plain text.
- Flags are not listed here. Before using a command for the first time in a session, run `ghostlink <cmd> --help`.
- Never guess an item or character. Resolve ambiguity with a read command or ask the user.
- `ghostlink equip` is the only game mutation. Run it only when the user explicitly named both the item and the character for this action.
- Never retry `equip` after a non-zero exit. Re-read state (`ghostlink sync`, then `ghostlink inspect-item`) and report.
- Never try to free inventory space, unequip elsewhere, or change plugs; the CLI refuses these by design.
- Run `ghostlink login` only when the user asks or after exit 10/11; it needs a human to authorize in a browser and waits for the callback.

## Decision Gates

| Intent | Command |
|---|---|
| Which characters exist, their IDs and light | `ghostlink list-characters` |
| Find owned items by name, type, perk, mod, stat, slot | `ghostlink search-inventory` |
| Full detail of one owned item | `ghostlink inspect-item` (concrete instance ID only) |
| Current subclass setup | `ghostlink subclass` |
| Unlockable supers, aspects, fragments | `ghostlink subclass-options` |
| Artifact points and active perks | `ghostlink artifact` |
| Saved in-game loadouts | `ghostlink loadouts` |
| Public weekly reset (no account needed) | `ghostlink weekly-reset` |
| Weekly progress for one character | `ghostlink weekly` |
| Data looks outdated or after an in-game change | `ghostlink sync` |
| Item names missing or definitions outdated | `ghostlink sync-manifest` |
| Equip an item on a character | `ghostlink equip` |
| Authorize this machine | `ghostlink login` |

`--character` accepts a class name (`hunter`, `titan`, `warlock`) on most commands, but `ghostlink search-inventory --character` requires the exact numeric character ID; get it from `ghostlink list-characters` first.

## Execution Steps

1. Pick the command from the table; check its `--help` if not yet seen.
2. Run it with `--json`. Reads refresh a stale profile automatically; use `ghostlink sync` only to force freshness.
3. On exit 0, answer from the JSON. Mention `metadata.checked_at` when freshness matters.
4. On non-zero exit, parse stderr. An object with `status` is an equip result: read `status` and `reason`. Otherwise read `error.code`. Then follow `references/exit-codes.md`.
5. For equip, quote the resolved item and character back to the user before running if either was inferred from earlier conversation.

## Output Contract

- State facts only from command output; say when a result is empty instead of inventing items.
- For equip, report `status`, each step, and `reason`. Exit 43 is applied but not yet visible: tell the user to check again later; it is not verified success.
- Never echo tokens, membership IDs, or raw provider errors.

## References

- `references/exit-codes.md` — error codes, exits, and the agent action for each.
