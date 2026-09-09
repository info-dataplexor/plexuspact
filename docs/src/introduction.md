# Introduction

PlexusPact is a shift-left data contract and validation engine: a single compiled Rust binary (powered by Polars/Arrow) that validates datasets against a versioned `contract.yaml` **at the source** — in the producing application's CI pipeline, at an API boundary, or on a file before upload — instead of after the bad data has landed in your warehouse, burned paid compute, and broken dashboards.

## Why shift left?

Bad code is blocked by a failing CI build. Bad data should be blocked the same way. Today, quality checks typically run *after* data lands in Snowflake or BigQuery — the data team gets paged for an incident an application team caused. PlexusPact moves the gate to where the breakage originates:

- A **data contract** is a versioned, machine-readable agreement between a data producer and its consumers: schema, quality rules, ownership, severity.
- `plexuspact check` validates any CSV, Parquet, NDJSON, or JSON dataset against that contract in seconds and exits with a CI-grade code.
- The failing output tells the producer exactly which column, which rule, which observed values, and sample rows — no postmortem archaeology.

## Why this tool?

- **Zero-boilerplate** where heavy Python frameworks are verbose: one YAML file, no notebooks, no plugin classes, no config sprawl.
- **Compiled-fast** where Python is slow: single static binary, no runtime dependencies, cold start under 50 ms.
- **Streaming and out-of-core** where Pandas hits OOM: a 10 GB CSV validates in constant memory on a 4 GB machine.
- **Deterministic**: same input, same contract, same result — the validation engine has no network client, so nothing outside your machine can change a verdict.

## What's in the box (Phase 1)

- `plexuspact check` — validate a file or stdin; human, JSON, JUnit, and self-contained HTML report output.
- `plexuspact init` — profile a dataset and draft a contract with inferred types and conservative commented suggestions.
- `plexuspact diff` — classify contract changes as breaking / semantic / non-breaking / cosmetic.
- `plexuspact validate-contract` — lint a contract without any data.
- `plexuspact push` — report a result to PlexusPact Cloud, if you want the runs [kept as history](cloud.md). `check` does it for itself once a key is set; without one, nothing leaves the machine.
- The [20-check core library](contract-reference.md#the-check-library): schema, value, and dataset-level checks with per-check `error`/`warn` severity.
- An [official GitHub Action](ci-recipes.md#github-actions) with PR annotations.

## Roadmap

**Phase 1.5** adds quarantine mode (route failing rows to a dead-letter file with per-row reasons — routing, never mutation) and AI-assisted authoring (`init --ai`, `explain --ai`), which is strictly opt-in and never in the enforcement path: `check` results are bit-identical with AI features disabled, offline, or unavailable. **Phase 2** adds SQL sources (DataFusion), Python/Node bindings, and the hosted registry (drift history, alerting, blast-radius lineage) — the commercial product. The CLI itself is Apache-2.0, free forever.

## Where to go next

- [Install](installation.md) the binary.
- Walk the [5-minute quickstart](quickstart.md) to your first green check.
- Keep the [contract reference](contract-reference.md) open while writing contracts.
