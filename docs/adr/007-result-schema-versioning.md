# ADR-007: Result schema versioning — additive-only within version 1

**Status:** Accepted · **Date:** 2026-07

## Context

The `RunResult` JSON document is consumed by parties we don't control and can't coordinate upgrades with: users' `jq` scripts and dashboards, the GitHub Action's annotation logic, CI systems parsing JUnit derived from it, and — in Phase 2 — the SaaS registry, for which it is the wire format (`plexuspact push` sends exactly this document). Every renderer (human, JUnit, HTML) is a pure function of it. Breaking this document breaks an unknown number of downstream consumers silently.

## Decision

- Every result carries `result_schema_version` (currently `1`) alongside `tool_version`.
- **Within version 1, changes are additive only.** New fields may be added at any level. Existing fields are never renamed, removed, retyped, or repurposed — a field's meaning is frozen the moment it ships.
- Consumers are instructed (docs, `result-schema.md`) to ignore unknown fields; our own renderers and the action follow that rule.
- Check `id`s are part of the contract: stable, deterministic (`<column>.<kind>[.<param>]`), so time-series consumers can key on them across runs.
- Metrics are captured **even for passing checks** (observed min/max/mean/null_ratio) — richer-than-needed today, but drift charting in Phase 2 is a pure storage problem as a result, requiring no CLI changes.
- A semantic change requires `result_schema_version: 2`, a deprecation window during which the tool can emit version 1 on request, and a migration note.

## Consequences

- `jq` pipelines and stored results survive every 0.x upgrade; the registry can ingest results from mixed CLI versions.
- Discipline cost: mistakes in a field's design cannot be fixed in place, only superseded by new fields — naming and typing reviews on result changes are strict.
- Snapshot tests lock the serialized form so accidental breakage is caught in PR review.
