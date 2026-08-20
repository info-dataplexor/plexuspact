# ADR-002: Workspace layering and the "contract crate has no Polars" rule

**Status:** Accepted · **Date:** 2026-07

## Context

Phase 2 requires reusing the core in very different hosts: PyO3/Node bindings, a long-running SaaS agent, possibly a DataFusion-backed engine. A monolithic binary crate would make each of those a rewrite. We also want the contract *model* (parse, validate, diff, JSON Schema) usable by tooling that must never pull in a DataFrame engine — editors, servers, the registry.

## Decision

The repo is a Cargo workspace of small crates with an enforced dependency direction:

```
cli → {contract, core, report};  core → {contract, io, engine};  engine → {contract, io}
```

(`core` does **not** depend on `report`; the `cli` composes `core` and `report` at the top. This is a cleaner arrangement than the original sketch — `report` is a pure consumer of `RunResult` and needs nothing from `core`'s orchestration — but the dependency *direction* is unchanged: renderers still sit above the data they render.)

- `plexuspact-contract` — YAML model, parsing, semantic validation, diff, JSON Schema. **Depends on nothing internal and never on Polars.** Checks here are declarative descriptions; only `engine` knows how to execute them.
- `plexuspact-io` — input abstraction: readers → LazyFrame, format sniffing, decompression.
- `plexuspact-engine` — check planning and execution against Polars.
- `plexuspact-core` — orchestration: `RunRequest → RunResult`; usable as a library (no `std::process::exit`, no printing).
- `plexuspact-report` — pure renderers of `RunResult` (human/json/junit/html).
- `plexuspact-cli` — the only bin: arg parsing, exit codes, terminal rendering.

The layering is checked in review and guarded by a CI job (`.github/workflows/ci.yml` → `layering`) that fails if `cargo tree -p plexuspact-contract` ever shows a Polars dependency.

## Consequences

- A Python binding or server links `core` and everything below it, skipping `cli`, with no refactor.
- The contract crate compiles fast and is embeddable anywhere (registry, editor tooling).
- Cost: more crates, some plumbing types crossing boundaries. Accepted — the invariant that `RunResult` is the single source of truth for all renderers depends on this separation.
- Violations (e.g. "just call Polars from the contract crate") are review-blocking regardless of convenience.
