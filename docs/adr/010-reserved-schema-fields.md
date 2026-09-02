# ADR-010: Reserved schema fields — `pii`, `classification`, `consumers`

**Status:** Accepted · **Date:** 2026-07

## Context

Contract schemas are painful to evolve once committed: contracts live in *users'* repos, are version-controlled, reviewed, and referenced by CI across many teams — a schema change fans out to all of them. Two Phase 2/3 product pillars need per-column and per-dataset metadata that no Phase 1 feature uses: **blast-radius analysis** needs to know who consumes a dataset (graph anchors), and **compliance evidence** (GDPR/DPDP/HIPAA reporting) needs PII and sensitivity tags on columns. Waiting until those features exist would mean asking every adopter to retrofit committed contracts later.

## Decision

The v1 schema reserves three optional fields from day one. In Phase 1 they are **parsed, semantically validated, and echoed into `RunResult` and reports — with no engine behavior.**

- Per-column `pii` — enum: `none | email | phone | name | address | national_id | financial | health | other`. A coarse taxonomy chosen to be assignable by a producer engineer without a privacy lawyer; finer taxonomies can extend the enum additively.
- Per-column `classification` — enum: `public | internal | confidential | restricted`. The de facto four-level corporate sensitivity ladder.
- Dataset-level `consumers` — list of `{ name, contact }`, declaring who depends on the dataset and whom to notify on breakage.

> **Amended 2026-08-30.** `consumers` is no longer reserved: it gained an optional `reads` list (the columns a consumer depends on) and now drives blast-radius analysis. The engine's side of the decision is unchanged — a consumer still changes no verdict and `check` still ignores the list — so this is an addition to the schema, not a reversal. The reservation stands for `pii` and `classification`. See [Consequences](#consequences) for the compatibility note this created.

**ODCS alignment:** field names and shapes are chosen to map onto the Open Data Contract Standard's equivalents where they exist (ODCS models data classification and consumer/stakeholder metadata), so the planned Phase 2 ODCS import/export (`From<OdcsContract>` conversions in the contract crate) is a translation, not a redesign, and contracts remain culturally familiar to teams already following the spec.

## Consequences

- Contracts committed today are forward-compatible: when blast radius and compliance evidence ship, tagged contracts light up without edits. **This paid off as designed** — blast radius shipped against the reserved `consumers` field, and every contract that had named its consumers lit up with no edit.
- **What the reservation did not cover: the shape inside the field.** Reserving `consumers` bought stability for the *key*. It bought nothing for the field's own shape — `Consumer` is `#[serde(deny_unknown_fields)]`, like every struct in the model, so each key added inside it is rejected outright by any CLI that predates it. `reads`, the first such key, costs nothing in practice: it ships in the same unreleased version as the rest of v0.1.0, so no binary exists that refuses it. The rule it establishes does have a cost, and v0.1.0 is where the installed base starts, so the policy is settled here rather than after it bites.
- **The strictness stays; the diagnostic improves.** Exempting `Consumer` from `deny_unknown_fields` was considered and rejected: a silently ignored key is precisely the failure this tool sells itself on catching, and one struct exempted would be an unexplainable special case for the next maintainer. The honest cost of strictness is that a newer contract *hard-fails* on an older CLI — which is the right failure, but only if the message names the right cause. So `parse.rs` now distinguishes a near-miss (a typo: say "did you mean", nothing more) from an unrecognizable key or value (which may be a schema this binary predates: name the upgrade path). "Remove the key" is never offered alone, because for a declaration like `reads` it is the one instruction that loses data — silently widening a consumer's blast radius with nothing downstream to mention it. See `unknown_key_note` and `unknown_value_note` in `crates/plexuspact-contract/src/parse.rs`.
- Honest-docs obligation: the reference and FAQ state plainly that these fields do nothing yet, to avoid implying enforcement that doesn't exist.
- Cost is near zero now (three optional serde fields + validation), and it buys schema stability where it is most expensive to lose.
- The enums ship `#[non_exhaustive]`-safe so values can be added within v1.
