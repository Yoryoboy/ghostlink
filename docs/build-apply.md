# Build apply: planning slice (BE-007a, incomplete)

There is **no CLI build apply command or MCP apply_build tool yet**. The current
slice supplies Manifest equipping labels and a pure equipment planner; it does
not dispatch actions. Existing move, equip, batch equip and in-game apply-loadout
commands are unchanged. This is not a completed build-application feature.

## Implemented boundary

- Migration 33 stores `equippingBlock.uniqueLabel` in a STRICT, cascading child
  table, refuses downgrade while populated, and invalidates Manifest freshness
  once on upgrade. The field lives beside existing equipping-block set metadata
  in `ArmorDefinition`. Missing and empty labels remain distinct. Run
  `ghostlink sync-manifest` after upgrading.
- Unique conflicts use the **label**, not Exotic rarity. DIM compares the same
  label on equipped items in different buckets (`item-move-service.ts:535-546`).
- `destiny_core::build_apply::plan_build_items` consumes resolved build items,
  inventory, definitions, characters and explicit capacity inputs. Resolution
  remains `build_analysis`'s responsibility; no same-hash replacements of build
  specifications are introduced.
- Ordered steps describe source replacement equips, Vault-mediated transfers,
  incidental space-making transfers and grouped arrival equips. Skips retain
  specification indices and typed reasons; already-satisfied items are separate.
  Sockets, mods and subclass application are explicitly `not_applied` with
  `sockets_mods_subclass_deferred_to_be_007b`.
- Planning clones its inventory and performs no repository or provider mutation.
  Failed preparation attempts discard their speculative local changes and steps.
  This is planning-time partial progress, **not** remote partial-failure handling.

## DIM evidence and adaptation

Evidence is from DIM commit `29c122e`, paths under `src/app/`:

| Decision | Evidence |
|---|---|
| Display-bucket order, source dequip before item moves, then arrival bulk equip | `loadout-drawer/loadout-apply.ts:355-604` |
| Exclude every resolved requested item from replacement/displacement choices | `loadout-drawer/loadout-apply.ts:417-436` |
| Prefer source character, then Vault, then other characters for replacements | `inventory/item-move-service.ts:216-257` |
| Prefer unconstrained label, matching type, rarity, then power; stable ties | `inventory/item-move-service.ts:1220-1279` |
| Prefer least-recently-played destination, preserve last Vault slot when possible | `inventory/item-move-service.ts:677-741` |
| Space-making candidate ranking | `inventory/item-move-service.ts:1138-1211` |
| Vault occupancy uses corresponding Vault bucket, not equipment-slot capacity | `inventory/stores-helpers.ts:65-66,101-107` |

Ghostlink has no DIM user-tag or manual-move data. Per owner decision, these
criteria are neutral: DIM's untagged item and no manual move. Definition search
`tags` are not DIM user tags and are never used as a substitute.

## Remainder before production exposure

The existing audited Postmaster-pull capacity policy covers only the eight
**character** equipment buckets (10 including equipped), not Vault capacity or
Vault-bucket correspondence. Production must not invent a Vault capacity. The
planner accepts optional explicit Vault evidence for equipment-only fixtures;
without it, cross-character staging reports `vault_capacity_unavailable`.
Unknown/unsupported occupants report `occupancy_unknown`, not a guessed free slot.
A general Vault bucket catalog and stack occupancy are still needed before this
input can be safely supplied in production. No network evidence was acquired.

Further work: complete DIM displacement ranking using saved in-game loadout
membership; optional clearWeapons/clearArmor final phase; Postmaster/stack
handling; single-item inline equip phase parity; source replacement preparation
and grouping across interleaved transfers. The current planner is equipment-only
and is **not an execution-ready DIM parity contract**.

Then wire revision/freshness-bound application planning and CLI `build apply
--dry-run` with shared text/JSON, prove zero action dispatches with fakes, and add
the executor plus MCP mutation tool. Execution must reuse optimistic action
adapters, pacing and typed Bungie rejection mapping, track dependency failures,
continue independent item work where DIM does, and never blindly retry or claim
provider-observed success. BE-007b remains separate.
