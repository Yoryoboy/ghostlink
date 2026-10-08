# Ghostlink MCP server (`destiny-mcp`)

`destiny-mcp` serves bounded read tools plus four default-enabled semantic
mutations over the Model Context Protocol stdio transport:

```text
get_postmaster    read committed Postmaster instances and anonymous stacks (no pull)
list_characters   list the signed-in account's characters
search_inventory  search Manifest definitions and concrete instances
inspect_item      inspect exactly one concrete instance by instance identifier
get_subclass      show equipped subclass configurations per character
get_subclass_options list available subclass options with unlock status
get_artifact      read live seasonal artifact state and per-character perks
get_loadouts      show saved in-game loadouts and configured equipment per character
weekly_reset      show the current public weekly rotation (same data as the CLI)
weekly            show one character's reliable weekly content and progress
move_item         move one concrete instance to the Vault or to a character
equip_item        transfer if necessary, equip on an explicit character, and verify
set_lock_state    lock or unlock one explicitly selected owned item and verify
pull_item         pull one genuine Postmaster equipment instance to its owner
pull_stack        pull an explicit amount of a non-instanced Postmaster stack
```

Mutation is composed by default in the product binary: its profile gateway and
durable projection are shared with the read path, and one explicit `move_item`
request is the whole intent authorization. There is no preview tool, confirmation
token or value, approval ceremony, or second MCP call. The single call plans the
move, forces a fresh revision-bound preflight, issues only the ordered transfer
steps, refreshes the projection after the action, and verifies the destination
or reports `applied_pending_provider_refresh` when provider-confirmed steps
still have an unchanged pre-action profile (see [Equip one item](cli.md#equip-one-item));
membership and credentials are resolved internally and are never accepted or
emitted through MCP.

`equip_item` accepts exactly `{"item":"Ace of Spades","character":"hunter"}`
(or exact instance/character ids). Both fields are required; unknown fields and
control characters are rejected. One call is the complete intent authorization,
with no separate token or approval ceremony. It uses the same shared equip
pipeline and JSON as `ghostlink equip`; see the [equip safety limits](cli.md#equip-one-item).
Both `equip_item` and `move_item` can return
`applied_pending_provider_refresh` with `isError=true`, never verified success.
`move_item` also has the `ghostlink move` CLI surface.

`set_lock_state` accepts exactly `{"item":"Ace of Spades","locked":true}`;
use `false` to unlock. Both fields are required, `locked` must be a boolean, and
unknown fields, controls, and overlong selectors are rejected before I/O.
Item resolution matches move/equip; ambiguous names require an instance ID.
One call is explicit intent for shared fresh preflight, at most one dispatch,
mandatory post-action refresh, and verification of `instance.locked`. Already
satisfied is a verified no-op, with no provider mutation. Vault items use one
owned character for the provider call; live validation remains pending.

Lock JSON matches `ghostlink lock` / `ghostlink unlock`: `status`, `item`,
requested `locked`, sanitized `attempt` and `reason`, `profile_refresh`, and
`observed` lock state. Only verified success/no-op have `isError=false`.
Refusal, incomplete, verification-failed and applied-pending-refresh results
have `isError=true`. No stale observation is presented as fresh after a failed
refresh. Never blindly retry uncertain outcomes or applied-pending-refresh;
sync later to confirm. See [Lock or unlock one item](cli.md#lock-or-unlock-one-item).
All publishing reads and mutations share the same server operation guard.

Install both binaries from a release and run `ghostlink` for setup (or
`ghostlink login` for sign-in only). Register `destiny-mcp` in your agent using
the wizard or the [manual instructions](../README.md#use-it-from-an-ai-agent).
No config file or environment exports are needed with official release defaults;
CLI and MCP share one keyring credential and automatically renew access tokens
in broker or confidential mode. See [configuration](configuration.md) for overrides.

To launch the stdio server directly:

```sh
destiny-mcp
# From a source checkout:
cargo run -p ghostlink --bin destiny-mcp
```

The process speaks newline-delimited JSON-RPC on stdout only. Every startup and
runtime failure is a sanitized message written to stderr; nothing else is written
to stdout. A missing stored credential fails closed with `authorization is
required`, a legacy raw-material credential with `reauthorization is required`,
and neither path guesses an expiry or falls back to an environment value.

## Postmaster observations

Read-only `get_postmaster` uses the same account scope, ordinary freshness policy
and committed snapshot as `ghostlink postmaster --json`. Omit `character` or use
`null` for all owned characters, including empty groups. A selector resolves an
exact ID first, then a unique case-insensitive class; ambiguity is an error.
Names are optional and do not require a loaded Manifest catalog.

| Input | Contract |
|---|---|
| Arguments | Object; `additionalProperties: false` |
| `character` | Optional string or null; string length 1–512 characters |
| Refused strings | Empty, whitespace-only, control-containing or oversized |
| Other fields | Rejected before service calls; no quantity, pull or membership input |

Example MCP tool call:

```json
{"name":"get_postmaster","arguments":{"character":"hunter"}}
```

`structuredContent` matches the CLI logical JSON exactly:

```json
{
  "kind": "postmaster",
  "metadata": {"checked_at": 10000, "freshness": "fresh", "revision": 2, "source": "profile"},
  "profile_currencies_coverage": "not_observed",
  "characters": [{
    "character_id": "123", "class": "Hunter",
    "instances": [{"instance_id": "456", "item_hash": 1, "name": null, "quantity": 1}],
    "stacks": [{"item_hash": 900, "name": null, "bucket_hash": 215593132, "quantity": 7}]
  }]
}
```

IDs are strings; hashes, quantities, timestamps and revisions are numbers.
Missing names/classes are null. Equal stack rows remain separate; do not sum
potentially overlapping sources. Stack buckets are actual runtime observations;
gear has no inferred bucket. Anonymous stacks have no instance ID, ordinal or
stable action selector. This tool exposes no pull action.

Metadata reports committed observations, not guaranteed live holdings or full
Postmaster coverage. Currency coverage is `not_observed`, `unavailable` or
`available` and qualifies component 103 only, independently of Postmaster coverage.
A failed refresh returns a sanitized categorized MCP error, never stale holdings.
Selector, storage and provider errors retain their shared application categories.

## Pull one Postmaster equipment instance

With explicit user intent, call:

```json
{"name":"pull_item","arguments":{"item":"700000000000000001"}}
```

`item` is the only field, required as a canonical positive int64 string
(1–9223372036854775807, no leading zeros). Unknown fields, numeric JSON IDs,
names, hashes, anonymous stacks, quantity and destination are unsupported.
Invalid input is rejected before account/service work. Use the genuine
`instance_id` from `get_postmaster.instances`, never a synthesized stack selector.

One call resolves authenticated membership, freshly derives the Postmaster owner,
qualifies non-destructive equipment with stored definitions and a freshly fetched
public Manifest index, and proves pinned target-bucket capacity. Unknown or changed
evidence refuses. It shares production gateway/repository clones and the existing
operation guard with publishing reads and sibling mutations, not a separate store.
No inventory clearing, arbitrary destination, preview, rollback or redispatch.

`structuredContent` is the same shared JSON as `ghostlink pull --json`:
`kind: "instanced_pull"`, `disposition`, `plan_id`, `dispatch`, independent
`receipt` and `failure`, and ordered `observations`. Each observation retains its
classification, failure and publication/target evidence separately. There is at
most one action and three post-action observations, two seconds between attempts.
Only `disposition: "verified"` has `isError=false`; `not_yet_observed`, `uncertain`
and `refused` have `isError=true`. Acknowledgement is not application proof; even
observed arrival cannot turn a refusal or timestamp failure into success.

Never blindly retry an unconfirmed outcome. Re-read state later and report receipt
separately from arrival. Local serialization excludes neither other processes nor
game activity; observations are not causal or atomic guarantees. Non-instanced stacks use `pull_stack`; live account validation is still pending.

## Pull a non-instanced Postmaster stack

With explicit hash, amount and owner intent, call:

```json
{"name":"pull_stack","arguments":{"item_hash":"7","amount":"3","character":"2"}}
```

All three fields are required decimal strings: `item_hash` is canonical uint32
(0–4294967295), `amount` positive int32 (1–2147483647), and `character` positive
int64 (1–9223372036854775807). No leading zeros (except hash `"0"`), signs,
whitespace, classes, numeric JSON values or extra fields. Invalid inputs are
rejected before I/O. Use committed stack observations, never synthesize instance
IDs or choose a destination.

Only qualified non-instanced account-wide Consumables/Modifications are supported.
Fresh planning and preflight prove eligibility and capacity; duplicate Postmaster
rows bound the amount by the smallest matching stack, not their sum. The adapter
uses the existing operation guard and production gateway/repository clones.

`structuredContent` is the shared `ghostlink pull-stack --json` shape: `kind:
"stack_pull"`, `disposition`, `plan_id`, `dispatch`, independent `receipt` and
`failure`, and ordered `observations`. One action, at most three fresh observations,
two seconds between attempts. Arrival requires an exact amount increase in the
destination and decrease in Postmaster. Only `verified` has `isError=false`;
refusal stays an error even if arrival is observed. Never blindly retry uncertain
or refused outcomes. Re-read later; observations are not causal or atomic proof.
Live account validation remains pending.

## Inventory search and inspection

`search_inventory` accepts top-level `perk_names` and `mod_names` arrays in
addition to the existing singular `perk_name` and `mod_name`. Terms combine
with AND, including the singular field, with a shared cap of eight terms per
kind. Each term resolves independently; any resolved name within it matches.
It also accepts `tier` (exact stored tier, case-insensitive), `location`
(`"vault"` or `"character"`), `equipped` (boolean), and `sort`
(`"Stat[:asc|desc]"`, descending by default). Sorting and name resolution follow
the [CLI rules](cli.md#name-matching-rules). Both `limit` and `instances.limit` default to 100 and
accept 1–1000. Search output includes `resolved.perk_terms`,
`resolved.mod_terms` and, when sorting, `resolved.sort`.

`get_subclass_options` accepts optional bounded `character` and `element`
(arc|solar|void|stasis|strand|prismatic), but no `locked` input. For example:
`{"character":"hunter","element":"strand"}`. Output matches
`ghostlink --json subclass-options --character hunter --element strand` exactly.
The status caveat and open `can_insert` question in the [CLI reference](cli.md#available-subclass-options) apply to this tool too;
raw `can_insert`/`enabled` flags are included. Each option includes optional
`description` (official Manifest tooltip), omitted when unavailable.

`get_subclass` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json subclass` exactly, including socket order and empty slots.
Each slot includes optional `description` (official Manifest tooltip), omitted
when unavailable. Descriptions prefer the item's own valid text, falling back to
its displayable sandbox perk effects in reference order, deduplicated and joined
with blank lines. Missing references are ignored; blank, overlong (2048 characters)
or invalid composed text is omitted. Sandbox acquisition is required and atomic
with Manifest publication; sync failure preserves the prior cache. Schema v14
invalidates v13 freshness so `sync-manifest` backfills unchanged upstream versions.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

`get_artifact` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json artifact` exactly, including points and equipped artifact sockets.
Each perk slot includes optional `description` (official Manifest tooltip), omitted
when unavailable.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

`get_loadouts` accepts an optional bounded `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json loadouts` exactly, including names, equipment, subclass
configuration, artifact perks and retained slot/socket indices. For example:
`{"character": "hunter"}`. Requires login and loaded Manifest definitions;
run `ghostlink sync-manifest` once after upgrading for loadout names. Each saved
plug includes optional `description` (official Manifest tooltip), omitted when
unavailable; cosmetic filtering is unchanged. Descriptions preserve multiline
text across all four tools.

Subclass slots, subclass options, artifact slots, loadout plugs and inspect
`plug_details` also include optional `clarity`: community research by
[Clarity](https://d2clarity.com), shown verbatim, including whitespace and
newlines. CLI JSON and MCP expose identical plug fields. Missing insights are
omitted. Exact hashes win; name fallback is restricted to the same semantic
plug kind, and unsupported kinds omit name fallback (exact hashes still work).
Unambiguous same-name, same-enhancement matches have no marker; an enhanced/base
fallback adds `clarity_match: "variant"`. Parent-item matches are preferred when
known, and ambiguous differing texts are never guessed. Official `description`
remains independent and unchanged.

When explaining effects, distinguish the official Bungie tooltip (`description`)
from community research (`clarity`), and cite Clarity when using its numbers.
Never invent values absent from both fields; missing `clarity` does not mean no
effect. Explicitly qualify variant text as an enhanced/base fallback, not exact
values for that variant, and never infer improved numbers.

Definition versions are automatic: MCP uses the Manifest stored in SQLite;
`DESTINY_DEFINITION_VERSION` is only an optional backward-compatible override.
MCP never downloads a Manifest inside a tool call (downloads can exceed client
timeouts). With no stored catalog, definition-dependent tools return the typed
`definitions_unavailable` error: `definitions not loaded; run ghostlink sync-manifest`.
Run that CLI command, or `ghostlink login` (which syncs an absent catalog after
successful authorization). The server can observe a subsequent local sync
without restarting.

`inspect_item` returns the same enriched names as CLI inspection:
`perk_names`, `mod_names`, `mods`, `archetype`, `available_perks`,
`available_mods` and `stats`, alongside the definition, instance and metadata.
It also adds `plug_details` entries with `hash`, `name`, `role` and optional
official `description`, community `clarity` and `clarity_match` (multiline text
preserved). Roles are `intrinsic`,
`perk`, `mod`, `available_perk`, `available_mod` and `archetype`. Every resolved
socketed plug with category `intrinsics` appears as `intrinsic` for weapons and
armor, instead of a `perk` detail for that plug. Other details come from the
existing name fields. Repeated equipped plugs are deduplicated by hash/role;
available names exclude equipped names and are unique by normalized name.
Intrinsic entries come first in socket order, then other equipped entries in
socket order, the first archetype, and available entries in socket order.
Missing descriptions and an empty `plug_details` list are omitted. Existing
name arrays are unchanged. `search_inventory` hits do not include descriptions;
call `inspect_item` for that detail.

## Offline stdio integration test

The real child-process stdio test is gated behind the non-default
`offline-fixtures` feature. It spawns a clearly test-only `offline-fixture`
binary that composes deterministic in-memory application test doubles, then
drives complete MCP sessions (initialize, discovery, the three reads, a
protocol failure, a sanitized tool failure, cancellation, and shutdown) and the
one-call mutation journey: one `move_item` request performs semantic planning, a
fresh preflight, the minimum transfer calls, a post-action refresh, and
destination verification with no second call. It also proves a repeated request
fails closed without replay, an already-satisfied destination is a verified
no-op, a changed preflight is a typed stale failure, a refused step is an honest
partial execution, and stdout is protocol-only with no secret sentinel present.
Pull scenarios additionally prove canonical validation, deterministic actual-bucket
arrival, receipt/disposition separation, pending/uncertain/refused outcomes and
failure without redispatch. The fixture's separate seeded read store is not proof
of production repository sharing:


```sh
cargo test -p destiny-mcp --all-features --test stdio
```

The fixture binary is never reachable from the product `destiny-mcp` binary and
requires no credential, network, durable database, or native keyring.
Equip child-process sessions additionally compare complete CLI/MCP JSON for
success, no-op, refusal, partial progress, uncertainty, pending provider refresh
and refresh/verification failure, with equipped non-equipment neighbors. These
are offline observations, not a live-provider verification claim.
Postmaster child sessions additionally refresh stale synthetic seeds through the
real query service and committed in-memory repository, preserving owned gear,
duplicate material/consumable stacks and runtime buckets. They exercise filtering,
all currency-coverage states, empty groups and refresh refusal with protocol-only,
secret-canary-checked streams. No user database or live account is accessed.

