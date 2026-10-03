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
- [ ] **ART-002 — Artifact query.** App query joining characters and resolving names.
- [ ] **ART-003 — CLI command.** `artifact` command, text and JSON output.
- [ ] **ART-004 — MCP tool and docs.** `get_artifact`, parity tests, README.

## Progress

- Branch `feature/artifact` stacked on `feature/subclass-config`.

## Next step

ART-002.
