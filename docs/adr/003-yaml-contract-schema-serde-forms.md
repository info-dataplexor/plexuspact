# ADR-003: YAML contract schema and serde dual forms

**Status:** Accepted · **Date:** 2026-07

## Context

The contract file is the product's primary UX surface: a backend engineer with no data tooling background must be able to read and write one in minutes. Candidates considered: JSON (no comments — disqualifying for `init`'s commented suggestions), TOML (poor nesting ergonomics for check lists), a custom DSL (parser cost, no editor support), and YAML. Prior art users already know: SodaCL and the open Data Contract Specification / ODCS are YAML.

There is also a syntax tension: parameterless checks want to read as words (`checks: [unique]`), parameterized ones as maps (`checks: [{min: 18}]`).

## Decision

- Contracts are **declarative YAML**, `apiVersion: v1`, deserialized with `serde`/`serde_yaml` into typed structs; column order is preserved (`IndexMap`) because report ordering matters to users.
- A **JSON Schema** is generated from the same structs (`schemars`) and committed at `schema/contract.v1.json`, giving editor autocomplete and validation via `yaml.schemas` with zero extra maintenance.
- Checks accept **both forms**: bare strings for parameterless checks (`unique`, `not_empty_string`) and single-key maps for parameterized ones (`{min: 18}`, `{format: email}`), each map optionally carrying `severity`. This is implemented with serde untagged/adjacent deserialization and covered by exhaustive parse tests — it is acknowledged as the fiddliest serde code in the project and is test-first.
- Parse errors are product: `miette` span-labeled diagnostics with line/column and a fix example (snapshot-tested).

## Consequences

- Ten-line contracts stay ten lines; editor tooling is free; ODCS alignment remains feasible (ADR-010).
- The dual-form deserializer is complex; the fallback (documented in the risk register) is requiring map form only, at a readability cost.
- YAML footguns (implicit typing) are mitigated by the semantic validation pass (`validate-contract`) rejecting surprising values with pointed errors.
