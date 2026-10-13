# Next release — DIM-style search and faster reads

Draft for the next release candidate. Live checks remain on the
[public-release checklist](../../odd/tasks/ghostlink-public-release.md).

## Highlights

- Search owned inventory with DIM-style queries in CLI and MCP, ANDed with
  structured filters. Unsupported filters fail explicitly instead of being
  silently ignored. `ghostlink search-inventory --filters` lists the registry's
  syntax and examples; see the [query reference](../cli.md#dim-style-query).
- Compact search tiles by default; request `--detail` / MCP `detail: true` for
  full hits. Continue owned results using `--offset` / MCP `offset` and returned
  `next_offset` (0–10000). Definition candidates are unchanged.
- Inspect by normalized exact owned name, not just instance ID. Ambiguity returns
  compact candidates; there is no fuzzy selection.
- Fresh MCP projection reads run on four concurrent slots without waiting for
  mutations. Profile syncs and mutations remain exclusive; read-triggered syncs
  are single-flight.
- Bungie read errors distinguish throttling, maintenance, reauthorization and
  API-key configuration. Process-wide adaptive pacing fails before dispatch for
  waits over 30 seconds with a retry time; actions are never automatically retried.
- Loadout results distinguish remote acknowledgement from local projection
  problems, including unresolved/skipped items and persistence failure. Effects
  are **projected locally**, not individually confirmed by Bungie. This release
  does no cross-character preparation before EquipLoadout: an item on another
  character or in the Postmaster is reported as unresolved (`on_other_character`,
  `in_postmaster`) instead of being projected. The seasonal artifact skip alone no
  longer makes an apply partial (exit 0), and a partial apply reports "some loadout
  items were not applied; see local_state" instead of a persistence failure.
- Actions report Bungie's typed refusals (for example `item_not_found` when an
  item moved behind Ghostlink's back, or `equipped`) even when Bungie sends them
  with an error status such as HTTP 500 or 400, instead of an untyped
  `uncertain` or `invalid_request`. Server errors without a recognized refusal
  code stay `uncertain`.
- Batch equip plans from the local projection like single-item equip, without a
  forced profile read before every batch.

```sh
ghostlink search-inventory 'is:weapon (is:solar or is:arc) -is:locked' --limit 5
ghostlink search-inventory 'type:"Hand Cannon"' --location vault --detail --json
ghostlink inspect-item --name "Ace of Spades" --json
```

## Read performance

Recorded offline release-build medians from the
[feature timing log](../../odd/tasks/ghostlink-dim-parity.md), using the harness
in `crates/destiny-adapters/tests/perf_baseline.rs`: 20,000 definitions, 600
instances, 300 stacks, three characters and 30 saved loadouts; nine samples.
These measure local SQLite/service work, not Bungie latency or live-account guarantees.

| Operation | Before → after |
| --- | --- |
| Definition name search (late match) | 131.5 → 0.9 ms |
| Publish one changed item | 80.1 → 5.9 ms |
| Owned stat-sorted search | 10.7 → 5.1 ms |
| Inspect item | 8.7 → 5.0 ms |

Stat-sorted owned search and inspection are roughly halved; other owned searches
also improve (text 2.3 → 1.6 ms, perk name 16.1 → 10.9 ms). Historical medians
are from the recorded runs, not timings re-measured for this documentation draft.

## Breaking / compatibility

- **Search hits are compact by default.** Consumers expecting nested `instance`
  and `definition` objects should opt into CLI `--detail` / MCP `detail: true`.
  Compact IDs are at `hits[].instance_id`. Candidates and inspection keep their
  existing shapes.
- **`apply_loadout.local_state` changes from a string to an object.** Read
  `status` (`updated`, `partial`, `failed`, `not_attempted`), `unresolved_items`,
  `skipped` and `persisted`. Other actions retain their string `local_state`.
  CLI exits 0/41/42 are unchanged; see [the output contract](../cli.md#apply-a-saved-loadout).

## Fixes

- Removed the rc.3 `apply_loadout` input schema's top-level `oneOf`, which the
  Anthropic API rejected and broke Claude client sessions. Advertised tool input
  schemas have no top-level `oneOf`/`anyOf`/`allOf`; selector exclusivity is
  enforced at runtime instead.
