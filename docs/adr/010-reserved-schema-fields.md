# ADR-010: Reserved schema fields — `pii`, `classification`, `consumers`

**Status:** Accepted · **Date:** 2026-07

## Context

Contract schemas are painful to evolve once committed: contracts live in *users'* repos, are version-controlled, reviewed, and referenced by CI across many teams — a schema change fans out to all of them. Two Phase 2/3 product pillars need per-column and per-dataset metadata that no Phase 1 feature uses: **blast-radius analysis** needs to know who consumes a dataset (graph anchors), and **compliance evidence** (GDPR/DPDP/HIPAA reporting) needs PII and sensitivity tags on columns. Waiting until those features exist would mean asking every adopter to retrofit committed contracts later.

## Decision

The v1 schema reserves three optional fields from day one. In Phase 1 they are **parsed, semantically validated, and echoed into `RunResult` and reports — with no engine behavior.**

- Per-column `pii` — enum: `none | email | phone | name | address | national_id | financial | health | other`. A coarse taxonomy chosen to be assignable by a producer engineer without a privacy lawyer; finer taxonomies can extend the enum additively.
- Per-column `classification` — enum: `public | internal | confidential | restricted`. The de facto four-level corporate sensitivity ladder.
- Dataset-level `consumers` — list of `{ name, contact }`, declaring who depends on the dataset and whom to notify on breakage.

**ODCS alignment:** field names and shapes are chosen to map onto the Open Data Contract Standard's equivalents where they exist (ODCS models data classification and consumer/stakeholder metadata), so the planned Phase 2 ODCS import/export (`From<OdcsContract>` conversions in the contract crate) is a translation, not a redesign, and contracts remain culturally familiar to teams already following the spec.

## Consequences

- Contracts committed today are forward-compatible: when blast radius and compliance evidence ship, tagged contracts light up without edits.
- Honest-docs obligation: the reference and FAQ state plainly that these fields do nothing yet, to avoid implying enforcement that doesn't exist.
- Cost is near zero now (three optional serde fields + validation), and it buys schema stability where it is most expensive to lose.
- The enums ship `#[non_exhaustive]`-safe so values can be added within v1.
