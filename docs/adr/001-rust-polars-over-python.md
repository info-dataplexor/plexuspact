# ADR-001: Rust + Polars over Python

**Status:** Accepted · **Date:** 2026-07

## Context

The product promise is shift-left validation running in *producer* CI — often an application team's Node/Go/Java repo with no Python environment — on files up to tens of GB, inside a CI time budget of seconds. Incumbent validators (Great Expectations, Soda, Pandera) are Python frameworks: heavy to install in foreign repos, slow on large files, and Pandas-based ones OOM on datasets that don't fit in memory. Non-functional requirements are explicit: ≥150 MB/s CSV throughput, <512 MB peak RSS streaming a 10 GB file, a single static binary <40 MB, <50 ms cold start, no panics on user input.

## Decision

Build the engine in **Rust** (stable toolchain, MSRV pinned in the workspace) with **Polars** (lazy/streaming API) as the DataFrame engine.

- Rust gives a single static binary with no runtime dependencies, memory safety, and predictable performance — the distribution story *is* the product wedge.
- Polars gives out-of-core streaming execution, Arrow-native memory, and best-in-class CSV/Parquet readers. Hand-rolling readers is where projects like this die; DataFusion is SQL-oriented and deferred to Phase 2 for remote/SQL sources.
- Go was rejected for its weaker DataFrame ecosystem (no Polars equivalent); Python was rejected because it *is* the incumbent form factor we are differentiating against.

## Consequences

- Zero-dependency install on Linux (musl static), macOS, Windows; viable in any CI.
- Junior-developer onboarding cost: Rust learning curve is budgeted explicitly in the implementation guide.
- We pin the exact Polars version and upgrade deliberately once per sprint — its API moves fast.
- Python/Node convenience is deferred to Phase 2 FFI bindings; the workspace layering (ADR-002) keeps that possible without a rewrite.
