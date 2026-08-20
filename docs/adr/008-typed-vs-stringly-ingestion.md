# ADR-008: Typed vs stringly ingestion — the Parquet asymmetry

**Status:** Accepted · **Date:** 2026-08

## Context

The check path must report *which rows* fail a `type` check and show the offending raw values. For text formats (CSV, NDJSON, JSON) the parser could be given forced dtypes, but a value that fails to parse would then be nulled (or abort the read) before the engine ever sees it — the evidence is destroyed at the ingestion boundary. Parquet is different in kind: it is a typed container, so a "wrongly-typed value inside a column" cannot exist; the only possible mismatch is a declared-vs-actual *schema* dtype mismatch for the whole column.

Treating both families identically would either lose row evidence for text formats or fake row evidence for Parquet.

## Decision

- **Text formats are read stringly for the check path**: every column is delivered as a `String` column (`TypingMode::Stringly` / `InputTyping::Stringly`). The engine casts each contract column to its declared dtype per batch, counts cast failures per row, and captures the offending raw strings as samples.
- **Parquet is delivered natively typed** (`InputTyping::Native`). A declared-vs-actual dtype mismatch is reported as a structural, file-level `type` failure with the actual dtype named — no row count, no samples, because no per-row failure exists.
- The asymmetry is deliberate and surfaced, not hidden: `type` outcomes from Parquet carry the actual/declared dtypes in `observed`, and the engine's tests pin both behaviors.
- Profiling (`init`) is exempt: it runs `TypingMode::Inferred` because drafting wants the parser's best-guess dtypes, not row evidence.

## Consequences

- Text-format `type` failures are debuggable to the row ("row 4123: `\"n/a\"` is not an int"); Parquet type failures are immediate and file-scoped, which matches how Parquet actually breaks.
- The engine casts text columns per batch, paying a cast per column per batch; this is the price of keeping raw values alive and is bounded by batch size.
- Consumers of `RunResult` must not assume every `type` failure has `rows_failed`/samples — Parquet ones legitimately have neither. `result_schema_version` 1 already models these as optional (ADR-007).
- If a future typed text container (e.g. Arrow IPC) is added, it follows the Parquet branch.
