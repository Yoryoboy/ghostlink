# Ghostlink MCP server (`destiny-mcp`)

`destiny-mcp` serves bounded read tools plus two default-enabled semantic
mutations over the Model Context Protocol stdio transport:

```text
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
`move_item` remains MCP-only: there is no CLI move command.

Run it with the shared read configuration present in the environment:

```sh
cargo run -p destiny-mcp
```

The process speaks newline-delimited JSON-RPC on stdout only. Every startup and
runtime failure is a sanitized message written to stderr; nothing else is written
to stdout. A missing stored credential fails closed with `authorization is
required`, a legacy raw-material credential with `reauthorization is required`,
and neither path guesses an expiry or falls back to an environment value.

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
when unavailable.
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

`inspect_item` returns the same enriched names as CLI inspection:
`perk_names`, `mod_names`, `mods`, `archetype`, `available_perks`,
`available_mods` and `stats`, alongside the definition, instance and metadata.
It also adds `plug_details` entries with `hash`, `name`, `role` and optional
official `description` (multiline text preserved). Roles are `perk`, `mod`,
`available_perk`, `available_mod` and `archetype`. Only plugs already surfaced
by the existing name fields are included, not otherwise-excluded exotic
intrinsics. Repeated equipped plugs are deduplicated by hash/role; available
names exclude equipped names and are unique by normalized name. Entries follow
equipped socket order, then the first archetype, then available socket order.
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
partial execution, and stdout is protocol-only with no secret sentinel present:

```sh
cargo test -p destiny-mcp --all-features --test stdio
```

The fixture binary is never reachable from the product `destiny-mcp` binary and
requires no credential, network, durable database, or native keyring.
Equip child-process sessions additionally compare complete CLI/MCP JSON for
success, no-op, refusal, partial progress, uncertainty, pending provider refresh
and refresh/verification failure, with equipped non-equipment neighbors. These
are offline observations, not a live-provider verification claim.

