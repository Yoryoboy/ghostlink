# Seasonal artifact state (WU-029)

## Objective and scope

Expose the current seasonal artifact state: artifact name, points acquired,
power bonus (profile scope) and, per character, points used, reset count,
unlocked tiers and active perks.

Out of scope: in-game loadouts, unlocked subclass options, persisting artifact
state in sqlite.

## Decisions

- Live-only read, like the weekly path: no sqlite persistence.
- Dedicated gateway call `GET /Destiny2/{t}/Profile/{id}/?components=104,202`
  with its own strict allowlist predicate. Sync query and weekly URL unchanged.
  Profile-level 202 returns `characterProgressions.data[cid].seasonalArtifact`
  for every character in the same response.
- Names resolve through existing item definitions: the artifact (`item_type`
  "Artifact") and perks (`plug_category` `artifact_perks`). No new manifest
  table (`DestinyArtifactDefinition` not needed for v1).
- Join to characters by `CharacterId` from the synced profile.

## Constraints and acceptance

- Bounded, strict decoding consistent with existing gateways.
- Invisible perks (`isVisible == false`) are not reported.
- CLI and MCP outputs match (parity test).
- Test-first; live smoke test against the real account at the end.

## Tasks

- [x] **ART-001 — Gateway and domain.** `ArtifactGateway::load_artifact`, `is_allowed_artifact_url`, bounded `decode_artifact` (8 chars/16 tiers/32 perks). RED→GREEN; workspace 1132 passed, clippy clean; live shape unverified until smoke. Commit `309ccc5`.
- [x] **ART-002 — Artifact query.** `ArtifactService::artifact` + `ArtifactOperations`, production `compose_artifact` in CLI runtime. RED→GREEN; workspace 1136 passed, clippy clean. Commit `b1daccf`.
- [x] **ART-003 — CLI command.** `ghostlink artifact [--character] [--tiers]`, JSON projection in `artifact_output.rs`. RED→GREEN; workspace 1142 passed, clippy clean. Live smoke OK: real 104/202 decoded (3 characters, 5 tiers, perk names resolved). Commit `eba34a6`.
- [ ] **ART-004 — MCP tool and docs.** `get_artifact`, parity tests, README.
- [ ] **ART-005 — Artifact name.** Live `artifactHash` (2894222926) is not an inventory item; load `DestinyArtifactDefinition` to resolve the name.

## Progress

- Branch `feature/artifact` stacked on `feature/subclass-config`.

## Next step

ART-004.
