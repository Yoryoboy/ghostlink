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

- For CLI operations, always add `--json`; never scrape human text. Exit 0 prints one JSON object on stdout; a non-zero exit prints one JSON object on stderr. Exceptions: `ghostlink login` first prints an `authorizing` instruction object, and `--help` / search `--filters` are plain text even with `--json`.
- Before using a CLI command for the first time in a session, run `ghostlink <cmd> --help`. For DIM query syntax/filter support, use `ghostlink search-inventory --filters` (registry-generated help, no inventory read).
- Never guess an item or character. Resolve ambiguity with a read command or ask the user.
- `ghostlink move` and `ghostlink equip` are CLI game mutations. Run them only with explicit user intent for this item and destination. Move requires exactly one explicit character or Vault destination; equip requires a character. Never select defaults.
- `ghostlink lock` and `ghostlink unlock` require explicit user intent for this item and requested state; MCP `set_lock_state` requires the same intent. Already satisfied according to local state is `verified_noop`; nothing was sent to Bungie. Vault items use one owned character for the provider call, not a destination selection.
- MCP `pull_item` and fallback `ghostlink pull` require explicit user intent for one genuine Postmaster equipment instance ID only. Destination is its committed Postmaster owner, never an arbitrary character. Names, hashes, stacks and quantity are unsupported; requires login and qualified stored definitions.
- MCP `pull_stack` with `{"item_hash":"7","amount":"3","character":"2"}` (all three canonical decimal strings, no extra fields), or fallback `ghostlink pull-stack <ITEM_HASH> --amount <N> --character <id|class>`, requires explicit hash, amount and Postmaster owner intent. Hash is canonical uint32 decimal; amount positive int32; MCP owner is positive int64, never a class. CLI accepts an exact ID or unique class. Only qualified non-instanced account-wide Consumables/Modifications are supported; do not synthesize stack IDs or select destinations. Requires login and stored definitions. Duplicate rows constrain amount by the smallest matching stack, never their sum.
- Never blindly retry `pull-stack`, `pull`, `move`, `equip`, `lock` or `unlock`, including after a non-zero exit. Re-read state (`ghostlink sync`, then `ghostlink inspect-item`) and report. For move/equip/lock/unlock/pull/pull-stack, `applied` exits 0 on full acknowledgement (MCP success), without claiming observed state. Acknowledged instanced and stack pulls persist locally and retain freshness; stack quantities are applied exactly once. Pull/pull-stack refused or transport-uncertain attempts exit 41; local persistence or invalidation failure exits 42 with the receipt retained. Neither emits provider-lag exit 43. Incomplete lock attempts exit 41, not success. Check later, do not repeat the mutation.
- Move already at its destination during planning is refused (exit 40), not a verified no-op. Report it without working around the refusal.
- `ghostlink update` and `ghostlink uninstall` are installation mutations, not game actions or reads. Run them only with explicit user intent; never automatically retry a failed or uncertain lifecycle command.
- Never try to free inventory space, unequip elsewhere, or change plugs; the CLI refuses these by design.
- Run `ghostlink login` only when the user asks or after exit 10/11; it needs a human to authorize in a browser and waits for the callback. On a machine without a browser, tell the user to run `ghostlink login start` and `ghostlink login complete` in their own terminal; never ask them to paste the callback address into the conversation, because it carries a one-use authorization code.
- `ghostlink handler install` and `ghostlink handler uninstall` change the user's desktop configuration; run them only when the user asks. `ghostlink handler status` is read-only.
- Explain perk, mod, aspect, fragment and ability effects only from output `description` (official Bungie tooltip) and optional `clarity` (community research). Attribute community numbers to Clarity; never invent numbers, percentages or durations absent from both fields. If neither gives an exact value, say it is unavailable.
- Missing `clarity` is not evidence of no effect. For `clarity_match: "variant"`, explicitly qualify the text as an enhanced/base fallback, not exact values for the inspected variant; never infer improved numbers.

## Bungie action rejections

Use the [shared reason table](references/exit-codes.md#definitive-bungie-action-rejections)
for every action. Exact numeric codes alone decide definitive rejection; status
text cannot. Report the stable `reason` and actionable `message`. Rejections
invalidate freshness, so the next action/read syncs; earlier acknowledged legs
remain applied. No automatic retry, replacement equip or rollback. Unknown
codes remain uncertain. Received refusals retain CLI 41 / MCP error.

## Decision Gates

| Intent | Command |
|---|---|
| Which characters exist, their IDs and light | `ghostlink list-characters` |
| Find owned items by name, type, perk, mod, stat, slot | `ghostlink search-inventory` |
| Full detail of one owned item, including what each perk and mod does | MCP `inspect_item` by `instance_id` or exact `name`; CLI `ghostlink inspect-item <INSTANCE_ID>` or `--name "<exact name>"` (never a definition hash) |
| Committed Postmaster instances and anonymous stacks (read only) | MCP `get_postmaster`; fallback `ghostlink postmaster` |
| Recover one genuine Postmaster equipment instance to its owner | MCP `pull_item`; fallback `ghostlink pull` |
| Recover an explicit amount of a non-instanced stack from one Postmaster owner | MCP `pull_stack`; fallback `ghostlink pull-stack` |
| Current subclass setup and what each equipped plug does | `ghostlink subclass` |
| Unlockable supers, aspects, fragments | `ghostlink subclass-options` |
| Artifact points and active perks | `ghostlink artifact` |
| Saved in-game loadouts (compact discovery) | MCP `get_loadouts`; fallback `ghostlink loadouts` |
| Full detail of one saved loadout | MCP `get_loadout`; fallback `ghostlink loadouts <slot-or-name>` |
| Apply one saved loadout on an explicit character | MCP `apply_loadout`; fallback `ghostlink apply-loadout` |
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

`ghostlink search-inventory --character` accepts an exact character ID or a unique case-insensitive class (`hunter`, `titan`, `warlock`), like most CLI commands. Exact IDs take precedence; unknown selectors and duplicate classes are errors. MCP `search_inventory.character_id` still requires an exact character ID; get it from `list_characters` first.

## Search and inspect recipe

Prefer MCP `search_inventory` with `query`, e.g.
`{"query":"is:weapon (is:solar or is:arc) -is:locked","location":"vault","limit":5}`.
Fallback:

```sh
ghostlink search-inventory 'type:"Hand Cannon" -is:locked' --location vault --json
ghostlink search-inventory 'stat:Range:>=30 power:>=440' --sort Range --json
ghostlink search-inventory --filters
```

Quote the whole query to protect spaces/comparisons; leading `-` is accepted.
Queries AND with structured flags. Explicit `and` > `or` > whitespace AND;
use parentheses for clarity. `-`/`not` negate. Quotes are phrases, not exact
names; use `exactname:` for exact matching. Comparisons use unsigned integers;
absent/zero power fails comparisons, but plain NOT includes such items.
An explicit empty query (`''` / `query: ""`) selects all owned items.
Unsupported DIM filters are errors. CLI query syntax errors exit 2
(`invalid_request`, kind/byte span/excerpt); MCP returns `invalid_query`
(kind/span/message, plus `supported_filters` only for unknown keys). Report and
correct or ask; never silently relax a query or retry it unchanged.

Search hits are compact tiles by default (`instance_id`, not nested `instance`),
with plugged perk/mod names and only filter/sort-referenced stats. Use CLI
`--detail` / MCP `detail: true` for the previous full shape. Continue owned hits
with returned `next_offset` via `--offset` / MCP `offset` (0–10000), keeping the
same query, filters, sort and limit. No `next_offset` means stop; definition
candidates are not paginated.

Inspect using exactly one instance ID or normalized exact owned name:
MCP `{"name":"Ace of Spades"}` or CLI
`ghostlink inspect-item --name "Ace of Spades" --json`. Never fuzzy-match.
On `ambiguous_match`, show compact candidates and ask the user which instance;
then inspect its `instance_id`. Inspect provides full detail and effect text.

## Equip several items

Require explicit user intent for every item and one target character. Prefer MCP
`equip_items` with `{"character":"hunter","items":["123456789","234567890"]}`
(2–17 items, no extra fields). Fallback: `ghostlink equip 123456789 234567890
--character hunter --json`. One selector retains single-item equip unchanged.
Use owned IDs or unambiguous names; never guess. Combined local validation rejects
repeated slots/items, wrong class, non-equippable items and exotic conflicts
including currently equipped slots not replaced. Transfers stop on first failure
before the single batch equip call. Report each item, slot, status and numeric
code; omitted `not_reported` results are failures, never success. All already
equipped according to local state is an all-no-op result; nothing was sent to Bungie. Full acknowledgement is applied (0 / MCP success),
not observed state; incomplete is 41 and local invalidation storage failure 42.
On location refusal ask the user to go to orbit or a social space. Read state
and reassess intent after failures; never blindly repeat a batch.

## Shared action state contract

For move, equip, equip_items, set_lock_state, pull_item, pull_stack and apply_loadout:
Plans from Ghostlink's local projection (synced only when missing, stale or invalidated).
Bungie is the final validator. Acknowledged actions update local state immediately;
rejections and uncertain outcomes mark it for refresh. Never retry an uncertain outcome.
Incomplete effects or local persistence failures also require refresh.

Except apply-loadout (typed object below), report the string `local_state` in
CLI JSON and MCP structured output: `updated` means
acknowledged effects persisted with freshness retained; `unchanged` means no-op
or refused before dispatch; `refresh_pending` means invalidated state requiring
the next ordinary read to re-sync. If Bungie has nothing newer yet, that read keeps the local state and reports
`freshness: stale`, never `fresh`, until a newer profile is published. Human output adds a short phrase only for
refresh pending. Compatibility status `verified_noop` means local state
(including Ghostlink's acknowledged actions) already matches; nothing was sent
to Bungie. This is not Bungie verification. Invalidated projections re-sync before
planning or deciding no-op.

## Read saved loadouts

Call MCP `get_loadouts` first (`{}` for all characters or
`{"character":"hunter"}`). It returns only saved slot/name, subclass name/element
and equipped item names per slot: no plugs or effect text. Fallback:
`ghostlink loadouts --json`.

For effects or configuration, select ONE result with MCP `get_loadout`, e.g.
`{"character":"hunter","loadout":3}` or
`{"character":"hunter","loadout":"PvE"}`. Fallback:
`ghostlink loadouts 3 --character hunter --json`. Names resolve exactly after
whitespace/case normalization, like apply-loadout. Unknown/ambiguous selectors
return typed errors and candidates; never guess. Omitted character searches all
characters and must resolve uniquely; specify character if ambiguous.

Detail includes full items, plugs, subclass configuration and artifact perks.
Read each plug's optional `insight` as a zero-based index into the loadout-level
`plug_insights` array. Those entries contain optional official `description`,
community `clarity` and `clarity_match`. Identical content shares one entry,
even across different hashes; differing parent-specific text remains distinct.
No index means no insight is available, not no effect. Both JSON shapes match
MCP exactly; no-selector text and `--detail` remain human-only presentation choices.
Both reads use saved loadouts cached with the profile (component 206), with a live
206 read only as a legacy fallback when that cache is unavailable.

## Apply saved loadouts

Require explicit user intent for the saved slot/name and character. Prefer MCP
`apply_loadout` with `{"character":"hunter","slot":3}` or
`{"character":"hunter","name":"PvE"}` (exactly one of slot/name, no extra fields).
Fallback: `ghostlink apply-loadout 3 --character hunter --json`. Character is
required: exact owned id or unique class; never default. Slots are one-based
1–12 from `get_loadouts` / `ghostlink loadouts`. Names use normalized exact matching
within that character; unknown/ambiguous names list candidates, never guess.
Requires login and loaded Manifest definitions. Ask the user to go to orbit or
a social space first (offline is also allowed); location refusal explicitly says
"go to orbit or a social space and retry". Ordinary sync resolves the cached
slot/name before one action (zero profile/206 reads while fresh and cached).
Report `kind: "apply_loadout"`, `character`, `slot`, `name`, `outcome`, `receipt`
and `reason`, plus the `local_state` object: `status` (`updated`, `partial`,
`failed`, `not_attempted`), `unresolved_items`, `skipped`, `persisted`.
Issues contain `{item: {instance_id, item_hash, bucket, name}, reason}`; hash,
bucket and name may be null. Report these local issues, not invented per-item
Bungie successes. `persisted: true` can mean no effects were written.
`applied` (CLI 0 / MCP success) means acknowledged, not observed. Known saved
items are **projected locally** as moved/equipped with same-bucket replacements;
complete persistence retains freshness. EquipLoadout pulls from the Vault but not
from another character or the Postmaster; those items are unresolved
(`on_other_character`, `in_postmaster`), never projected. Missing or unreachable items,
subclasses not held on the target or rejected effects apply the rest and invalidate.
The seasonal artifact skip (`unsupported_bucket`) alone is not partial.
Held subclasses switch locally, unequipping the previous subclass without
invalidation; single-item equip validation is unchanged. Mods/plugs, including
subclass aspects/fragments, stay unchanged (DIM/Bungie limitation). Non-applied receipts invalidate
and fail (41); an applied receipt with incomplete local effects or persistence
failure is 42, retaining the receipt: partial reports "some loadout items were not
applied; see local_state", persistence failure "local state could not be updated after the action".
No post-action request, provider-lag exit 43 or automatic retry. After invalidation,
read later and reassess intent, never blindly repeat `apply-loadout`, including
after nonzero exit.

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
The ID must be canonical positive int64, without leading zeros. Ordinary sync
uses the committed projection and non-destructive equipment definitions, without
per-action Manifest capacity evidence. Pinned capacity counts character-held
instances (including equipped) by definition bucket. Known full refuses locally;
unknown occupant definitions allow a blind attempt with Bungie as final validator.
Numeric error 1642 is destination full; provider text alone is never sufficient. One action maximum, no post-action profile request, wait, redispatch or
rollback. Bungie acknowledgement yields `disposition: "applied"` (CLI 0 / MCP
success), not observed arrival. Acknowledged pulls update local character inventory
without equipping, advance revision and retain freshness. Incomplete attempts or
persistence failure invalidate so the next ordinary read syncs.
Transport uncertainty stays `uncertain`; refusals stay `refused` (CLI 41 / MCP
error). Local persistence or invalidation failure is `local_state_failed` (CLI 42 / MCP
error), retaining the receipt and reporting "local state could not be updated
after the action". No provider-lag exit 43 or post-action `observations` field.
Caller-local serialization does not cover other processes or game activity.
Anonymous stacks use MCP `pull_stack` with required `item_hash`, `amount`, and
`character` decimal strings (uint32, positive int32, positive int64 respectively),
or fallback `pull-stack` (CLI owner accepts an exact ID or unique class); MCP
accepts no names, classes, instance IDs or extra fields. Only
qualified non-instanced account-wide Consumables/Modifications are supported.
Duplicate rows bound amount by the smallest matching stack. Ordinary sync plans
and executes from committed stacks; fresh state needs no profile acquisition or
Manifest capacity fetch. Pinned audited capacity is not version/artifact qualified;
locally full refuses with unchanged unique-stack arithmetic. Bungie is the final
validator (numeric 1642 is destination full; unknown codes remain uncertain).
Acknowledgement applies the requested quantity locally once, decrements Postmaster,
fills/splits profile stacks, advances revision and retains freshness. Incomplete
receipts or persistence failures invalidate; no retry. Dispatch retains only
pre-action totals, not observed post quantities. Live checks remain pending.
Never retry uncertain or refused outcomes blindly; sync, re-read and reassess
explicit intent before another action.

## Installation Update

- Supported only on Linux x86_64 GNU, using the validated official sibling updater. Unsupported installations refuse mutation; use the original installation method rather than work around validation.
- Windows returns `unsupported_operation` without launching or deferring anything. Tell the user to close Ghostlink, stop `destiny-mcp`, and run the official sibling `.\ghostlink-update.exe` from its installation directory in a separate PowerShell session.
- Update preserves configuration, data and credentials. Restart MCP hosts afterward to load `destiny-mcp`.
- `status: updater_completed` means only the updater completed, not that a requested version was installed; `installed_version` is null. No atomic paired update or rollback is guaranteed. After failure or an uncertain outcome, inspect the installation first; never automatically retry.

## Installation Uninstall

- Preview with `ghostlink uninstall --dry-run --json`; it is strictly read-only, including no lock creation. Mutation in JSON requires `ghostlink uninstall --yes --json`, only after explicit user authorization. No purge is supported.
- Linux x86_64 GNU official receipt-owned installations only. Missing updater is optional; source/manual/unknown installations refuse. Windows refuses before lookup/spawn: ask the user to stop MCP hosts and active login, close Ghostlink, then inspect the receipt and remove only owned files/integrations from a separate PowerShell session (or use the original package manager).
- Preserve all configuration, databases, credentials, runtime state, external/custom paths, unknown files/backups, Cargo directories and PATH/shell config. Config is never loaded to resolve overrides. Stop MCP hosts and active login first; no process inactivity guarantee or termination.
- Exact bundled skill identity is not installation provenance. Modified/older skills remain. Only exact installation-owned OpenCode commands and proven URI entries are eligible; name-only Claude/Codex/Pi registrations always require manual followup. Handler skips preserve handler files but allow owned binary/updater/receipt removal with exit 41; handler mutation errors, agent filesystem failures or changed snapshots still retain remaining binaries/receipt.
- Handler skips appear in read-only previews as `expected_status: "partial"` and `warnings` (JSON and human). Use the concrete paths in `manual_followups`: select an installed replacement with `xdg-mime default <other>.desktop x-scheme-handler/ghostlink`, or remove Ghostlink from scheme lines in the listed mimeapps.list, preserving foreign associations. Inspect higher-priority desktop-specific MIME files if needed; verify with `xdg-mime query default x-scheme-handler/ghostlink`. Only after Ghostlink is no longer selected and ownership is verified, delete the listed desktop entry and previous-handler record. Never delete foreign files.
- Exit 0 can mean preview, cancelled, or removed_with_manual_followups, never unconditional removal of all integrations. Exit 41 partial_failure is one stderr JSON object containing both error and report, including completed/remaining per-path outcomes. Individual changes are not transactional; no rollback. Lock metadata remains intentionally. Report manual followups and preserved locations; never blindly retry.

## Descriptions

- Five plug surfaces expose optional official `description` and community `clarity`: subclass slots, subclass options, artifact slots, saved-loadout detail's referenced `plug_insights`, and inspect `plug_details` (`hash`, `name`, `role`, optional `description`, `clarity`, `clarity_match`). Loadout summaries contain no effect text: call `get_loadout` after `get_loadouts`. Missing fields mean no text is available, not no effect.
- Search returns compact tiles (or full hits with `detail`), without effect descriptions. To explain perks, search first, then inspect the chosen instance.
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
4. On non-zero exit, parse stderr. An object with `status` is a move, equip, lock or unlock result: read `status` and `reason`. For `kind: "instanced_pull"` or `kind: "stack_pull"`, read `disposition`, independent `receipt` and `failure`; there are no post-action observations. For `kind: "apply_loadout"`, read `outcome`, `receipt`, `reason`, typed `local_state` (status/issues/persisted) and any name-resolution `candidates`. For refused `kind: "loadout"`, read `code`, `reason` and `candidates`. Otherwise read `error.code`. Then follow `references/exit-codes.md`.
5. For mutations, quote the resolved item and destination (move/equip) or requested lock state (lock/unlock) back to the user if inferred from earlier conversation; obtain explicit intent before acting. For pull, confirm the exact genuine instance ID and its observed Postmaster owner.

## Output Contract

- State facts only from command output; say when a result is empty instead of inventing items.
- For move or equip, report `status`, each step, and `reason`; never claim rollback after partial progress. Move/equip `applied` means every step was accepted (exit 0), not observed location/equipped state. Acknowledged effects persist locally with freshness retained; incomplete attempts require refresh. Neither emits provider-lag exits 42/43; uncertain and partial results still fail. Update exit 43 instead means `lifecycle_busy`.
- For lock/unlock, report `status`, requested `locked`, `attempt`, `observed` and `reason`. `applied` means Bungie acknowledged the requested state (exit 0 / MCP success), not post-action observation; only no-op has committed-local `observed`. There is no post-action profile request; complete local persistence retains freshness. Exit 41 is incomplete; 42 is fail-closed local invalidation storage failure, not provider lag. Lock no longer emits lag exit 43. Never blindly retry.
- For pull/pull-stack, report `disposition`, `receipt` and `failure`. `applied` means acknowledged (CLI 0 / MCP success), not observed arrival or quantities. No post-action request/wait or observations. Acknowledged instanced and stack pulls persist locally and retain freshness. Refused/transport-uncertain attempts exit 41 and invalidate; local persistence or invalidation failure exits 42, retaining the receipt. Sync and inspect/Postmaster later; never blindly repeat. Confirm the exact instance or hash, amount and owner before acting.
- Never echo tokens, membership IDs, or raw provider errors.

## References

- `references/exit-codes.md` — error codes, exits, and the agent action for each.
