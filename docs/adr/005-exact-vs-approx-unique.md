# ADR-005: Exact vs approximate `unique`

**Status:** Accepted · **Date:** 2026-07

## Context

`unique` is a stateful check: it needs memory across batches, unlike streaming-aggregatable checks. Exact detection requires holding one entry per distinct value; hashing values to u64 (`ahash`) bounds that at ~8 bytes × cardinality, which is fine for typical key columns but reaches ~800 MB at 100 M distinct values — violating the <512 MB streaming memory target at the extreme. Approximate structures (HyperLogLog) run in constant memory but cannot name duplicate rows and have an error margin — a false "unique: failed" on clean data would destroy trust in a CI gate, and a validator that silently *approximates* by default is arguably lying.

## Decision

- **Exact is the default.** `unique` maintains an `ahash` set of value hashes per column; duplicate samples capture the first re-seen occurrence so failures show real rows. We hash values (not raw strings) to bound memory, and document the negligible u64 collision probability in code.
- **`approx: true` is an explicit opt-in** per check (`{ unique: { approx: true } }`), switching to HyperLogLog: constant memory, no sample rows, documented error bound. Intended for very high-cardinality columns.
- The engine warns when an exact `unique` crosses a cardinality threshold, suggesting `approx: true`, rather than silently degrading.
- `unique_ratio_min` follows the same mechanism (distinct estimate over row count).

## Consequences

- Default behavior is trustworthy and debuggable (row-level duplicate samples); memory cost is documented and predictable.
- Users with 100M+-cardinality columns must make one explicit choice; the tool never chooses approximation for them.
- The exact/approx split is visible in `RunResult.checks[].params`, so results are interpretable after the fact.
