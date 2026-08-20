# ADR-009: Duplicate accounting — second-and-later occurrences fail

**Status:** Accepted · **Date:** 2026-08

## Context

When `unique` finds the value `"a"` three times in a column, how many rows failed? Defensible answers: all three occurrences (3), the "extra" occurrences (2), or one per duplicated value (1). The choice changes `rows_failed`, the failure ratio users alert on, and which rows appear as samples. It also has to hold up in streaming execution, where the engine sees batches in order and cannot retroactively mark the first occurrence once a later duplicate appears.

## Decision

- **`rows_failed` counts second-and-later occurrences.** The first occurrence of every value is legitimate; each re-observation is the violation. Three `"a"`s → `rows_failed = 2`.
- Samples capture re-seen occurrences (the rows the streaming engine can point at when the violation happens), and the failure message names the first duplicated value observed, e.g. `2 duplicate value(s), e.g. "a"`.
- `observed.distinct_count` reports the true distinct count, so `rows_evaluated − distinct_count = rows_failed` is an invariant of exact mode.
- Approx mode (ADR-005) has no per-row knowledge, so it reports the same quantity by construction: `rows_failed = rows_evaluated − distinct_estimate` when the estimate falls below the tolerance threshold, flagged with `observed.approx = true` and a `≈` message. Exact and approx therefore estimate *the same number*.

## Consequences

- `rows_failed / rows_evaluated` behaves as a duplication ratio (0 when unique, → 1 as the column collapses to one value), which is the intuition alerting rules want.
- The first occurrence never appears in samples; users tracing a duplicate get the re-seen rows, and can find the original by value.
- Streaming stays single-pass with no retroactive bookkeeping.
- Any future check that counts "repeats" (e.g. composite-key uniqueness) must use the same second-and-later convention for consistency.
