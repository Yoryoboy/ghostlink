# Destiny MVP Architecture

## Objective

Turn the product vision in `Destiny AI Companion — Objetivo, alcance y arquitectura.md` into an evidence-backed, implementable architecture and work breakdown for the first vertical MVP.

## Problem

The vision defines the target capabilities and long-term roadmap, but the first implementation still needs explicit boundaries, contracts, security decisions, verified external integration assumptions, and reviewable work units.

## Why

A narrow architecture baseline prevents the greenfield repository from coupling Bungie transport details, local persistence, CLI presentation, and MCP presentation before their responsibilities are understood.

## Scope

- Validate Bungie OAuth and official MCP Rust SDK assumptions against current primary sources.
- Define the MVP's boundaries, domain model, component responsibilities, workflows, failure behavior, security posture, and acceptance criteria.
- Produce an ordered implementation breakdown suitable for reviewable work-unit commits.
- Keep implementation code and project scaffolding out of this feature.

## Constraints

- Bungie remains the source of truth; local SQLite data is a cache and search index.
- CLI and MCP must remain thin adapters over one Rust core.
- Secrets must not be exposed to MCP clients, model context, logs, or SQLite.
- Mutating inventory actions must refresh before execution and verify afterward; CLI and MCP use the same mutation policy and executor.
- Technical artifacts are written in English.
- Use primary official sources for material external integration claims.
- This is Organic Driven Development, not SDD/OpenSpec.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 350 authored documentation lines, excluding the existing vision document.
- Chain strategy: not required unless the forecast or running count exceeds approximately 400 authored lines.
- Engram mirror topic: `odd/destiny-mvp-architecture/tasks`.
- Engram mirror status: pending because the local Engram provider reported an ownership mismatch.

## Effective checks

- TDD mode: not applicable to this documentation-only feature.
- Test runner: none yet; the repository contains no implementation scaffold.
- Required checks: primary-source traceability, internal consistency with the vision, bounded MVP scope, explicit open questions, and clean Markdown readback.

## Tasks

- [x] **DMA-001 — Bootstrap the feature branch and baseline repository**
  - Route: inline; repository-state operation and one tracking file.
  - Outcome: created `feature/destiny-mvp-architecture` and committed `.gitignore`, the source vision, and this feature document as the recoverable baseline.
  - Check: `git status --short --branch` reported a clean feature branch; `git show --stat --oneline --summary HEAD` reported the three intended files.
  - Evidence: commit `108b08c` (`docs: establish Destiny companion vision`).

- [x] **DMA-002 — Validate official integration assumptions**
  - Route: delegated research outcome captured from the parent-supplied primary-source evidence.
  - Outcome: created `docs/integration-assumptions.md` with verified Bungie OAuth constraints, verified MCP Rust SDK facts, explicitly labeled inferences, unresolved deployment/version gaps, and a source register.
  - Check: read back the artifact for scope/date, primary-source traceability, confidence/freshness notes, and the required distinction between provider facts and project recommendations.
  - Evidence: commit `1f32b0f` (`docs: validate Destiny integration assumptions`).

- [x] **DMA-003 — Write the MVP architecture**
  - Route: delegated to `gentle-ai-worker`; non-trivial architecture document informed by the research, followed by a bounded correction from independent verification.
  - Outcome: created `docs/mvp-architecture.md` with decisions, boundaries, contracts, workflows, security, failure semantics, and acceptance criteria for the public-client vertical MVP with plan-bound MCP mutation confirmation.
  - Check: `git diff --check` passed; independent correction verification passed with no findings.
  - Evidence: initial commit `6cfd76d` (`docs: define Destiny MVP architecture`); correction commit `b1f9e29` (`docs: reconcile MCP mutation architecture`).

- [x] **DMA-004 — Write the implementation work breakdown**
  - Route: delegated to `gentle-ai-worker`; bounded planning document.
  - Outcome: created `docs/mvp-implementation-plan.md` with ordered work units, dependencies, narrow ownership surfaces, test strategies, acceptance checks, exclusions, review notes, traceability, and readiness guidance.
  - Check: documentation plan read back for dependency-correct vertical progress, all required architecture work areas, explicit public OAuth/MCP confirmation constraints, implementation-time provider revalidation, and architecture acceptance-criteria mapping.
  - Evidence: commit `a8cd132` (`docs: plan Destiny MVP implementation`).

- [ ] **DMA-005 — Review and close the architecture package**
  - Route: delegated to `gentle-ai-verify`; independent documentation verification because verification commands are delegated by policy.
  - Outcome: resolve inconsistencies and report decisions, evidence gaps, and the recommended first implementation unit.
  - Check: focused verification plus final repository readback.
  - Evidence: pending.

## Acceptance criteria

- Current official integration assumptions are separated from recommendations and unknowns.
- The architecture defines one vertical MVP and explicitly excludes later roadmap phases.
- Core, CLI, MCP, Bungie, Manifest, SQLite, and credential boundaries are unambiguous.
- Inventory mutation planning, execution, partial failure, and verification behavior are specified for both CLI and MCP, including plan-bound explicit confirmation.
- The work breakdown can be implemented sequentially without rediscovering architectural decisions.
- Every completed task records observed checks and a work-unit commit identity.

## Progress

- User selected architecture-first MVP design.
- Repository began with `.gitignore` and the source vision as untracked files and no commits.
- User explicitly authorized an initial commit containing the documents.
- `feature/destiny-mvp-architecture` now has baseline commit `108b08c`; its committed tree contains the intended three files and was clean immediately afterward.
- Baseline evidence was recorded in follow-up commit `38ca47b`.
- DMA-002 research is now recorded in `docs/integration-assumptions.md`; implementation-time revalidation remains required for the moving MCP `main` branch, while DMA-003 resolves the MVP as a public-client deployment with an exact configured callback contract.
- DMA-003 architecture is recorded in `docs/mvp-architecture.md`; commits `6cfd76d` and `b1f9e29` capture the initial design and MCP-mutation correction, and independent correction verification passed with no findings.
- DMA-004 implementation plan is recorded in `docs/mvp-implementation-plan.md`; the plan defines ordered work units, scaffold-established commands, test categories, review slicing, traceability, and remaining implementation-time revalidation blockers. Evidence is pending commit.

## Next step

Proceed to DMA-005: review and close the architecture package.
