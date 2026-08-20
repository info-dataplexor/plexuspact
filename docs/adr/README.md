# Architecture decision records

One page each: **Context / Decision / Consequences**. When you face a choice the docs don't answer, propose an ADR here rather than deciding silently in code (see CONTRIBUTING.md).

| # | Title | Status |
|---|---|---|
| [001](001-rust-polars-over-python.md) | Rust + Polars over Python | Accepted |
| [002](002-workspace-layering.md) | Workspace layering and the "contract crate has no Polars" rule | Accepted |
| [003](003-yaml-contract-schema-serde-forms.md) | YAML contract schema and serde dual forms | Accepted |
| [004](004-exit-code-convention.md) | Exit-code convention | Accepted |
| [005](005-exact-vs-approx-unique.md) | Exact vs approximate `unique` | Accepted |
| [006](006-telemetry-policy.md) | Telemetry policy — opt-in only, none in the MVP | Accepted |
| [007](007-result-schema-versioning.md) | Result schema versioning — additive-only within version 1 | Accepted |
| [008](008-typed-vs-stringly-ingestion.md) | Typed vs stringly ingestion — the Parquet asymmetry | Accepted |
| [009](009-duplicate-accounting.md) | Duplicate accounting — second-and-later occurrences fail | Accepted |
| [010](010-reserved-schema-fields.md) | Reserved schema fields — `pii`, `classification`, `consumers` | Accepted |
| 011 | Quarantine semantics (reserved — to be written before Phase 1.5 implementation) | — |
| 012 | AI privacy & determinism boundary (reserved — to be written before Phase 1.5 implementation) | — |
