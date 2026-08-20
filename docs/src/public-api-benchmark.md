# Public-API capability & reliability benchmark

This report captures how the PlexusPact engine behaves when pointed at real,
unmodified responses from public REST APIs. The goal was threefold: prove the
engine can validate live-shaped data, quantify the overhead validation adds to a
data pipeline, and surface any reliability or scalability limits before they
reach production.

All numbers below come from the release binary
(`target/release/plexuspact.exe`) on Windows 11 (x86-64, GNU toolchain). Each
endpoint was fetched once (respectful of rate limits) from the
[apipheny.io free-API list](https://apipheny.io/free-api/).

## Capability: what validated cleanly

14 endpoints were exercised through the full `init` (profile + draft contract)
then `check` (validate) path. **11 of 14 validated end-to-end with exit 0.**

| API | Shape | Cols | Rows | Checks | `check` time |
|-----|-------|-----:|-----:|-------:|-------------:|
| jsonplaceholder/users | array, nested address/company | 8 | 10 | 18 | 72 ms |
| jsonplaceholder/posts | flat array | 4 | 100 | 10 | 76 ms |
| jsonplaceholder/comments | flat array | 5 | 500 | 12 | 68 ms |
| jsonplaceholder/todos | flat array | 4 | 200 | 10 | 64 ms |
| coingecko/markets | numeric-heavy, 26 fields | 26 | 100 | 52 | 67 ms |
| randomuser (`.results`) | deeply nested, extracted | 12 | 25 | 25 | 65 ms |
| usgs earthquakes (`.features`) | GeoJSON, extracted | 4 | 42 | 9 | 62 ms |
| pokeapi (`.results`) | extracted | 2 | 100 | 6 | 67 ms |
| open.er-api (USD) | object-of-scalars | 11 | 1 | 24 | 67 ms |
| agify | single object | 3 | 1 | 8 | 62 ms |
| ipify | single field | 1 | 1 | 4 | 79 ms |

Nested top-level objects on otherwise-tabular rows (e.g. `address {}` and
`company {}` on jsonplaceholder/users) are flattened by the JSON reader and
validate without intervention. The engine handled numeric-heavy payloads
(coingecko, 26 columns / 52 checks) at the same cost as trivial ones.

## Scalability: is validation a pipeline bottleneck?

A real coingecko response was replicated to NDJSON at increasing scale and
validated (26 columns, 52 checks):

| Rows | Size | Wall | Throughput | Row rate |
|-----:|-----:|-----:|-----------:|---------:|
| 10,000 | 8.5 MB | 235 ms | 36 MB/s | 43k rows/s |
| 100,000 | 84.7 MB | 1,770 ms | 48 MB/s | 57k rows/s |
| 500,000 | 423.7 MB | 9,081 ms | 47 MB/s | 55k rows/s |

**Conclusion: validation is not a bottleneck.** Throughput is flat-to-improving
as data grows (fixed startup amortizes away), and memory stays constant — the
engine streams in batches rather than materializing the whole frame. The
per-invocation floor is ~60–80 ms and is dominated by process startup, not by
the checks themselves; the marginal cost is roughly **18 µs/row** for 52 checks
across 26 columns. For any dataset large enough to matter, validation is a small,
linear, predictable fraction of total pipeline time.

> Practical guidance: the fixed ~65 ms startup means you want **one `check`
> invocation per dataset**, not one per row or per micro-batch. In streaming
> pipelines, validate at the batch boundary.

## Reliability findings

### 1. Nested array/object columns now fail cleanly (fixed)

Three response shapes previously produced an opaque **internal error** (exit 3,
`cannot cast List type …`):

- **Array-valued fields** — `universities/search` (`domains`, `web_pages`),
  any list column.
- **Nested object rows** — `restcountries` (`name` is an object).
- **Object-wrapped payloads validated without extraction** — `randomuser`,
  `usgs`, `pokeapi` when the raw envelope (`{ results: [...] }`) is passed
  directly, so the array lands in a single `List` column.

These are legitimate user-input situations (pointing the tool at a raw API
envelope), not engine bugs, so they should be **actionable user errors, not
internal errors.** The engine now detects nested `List`/`Struct` dtypes before
casting and returns:

```
✗ column `web_pages` contains nested array values; plexuspact validates flat,
  tabular columns — select or flatten the field before validating (e.g. extract
  the record array, or map the nested field to a scalar)
```

with **exit code 2** (user error). This is enforced on both the `init`/profile
path and the `check`/validate path, and is covered for array, object, and
object-wrapped-envelope shapes.

### 2. Object-wrapped arrays — inline `--json-path` selector (fixed)

Many APIs wrap their records in an envelope: `{ "results": [...] }`,
`{ "features": [...] }`, `{ "data": [...] }`. PlexusPact validates a table, so
the record array must be reached first. This is now a **single flag** on both
`init` and `check`:

```
plexuspact init  response.json --input-format json --json-path results
plexuspact check response.json --input-format json --json-path data.items \
  --contract contract.yaml
```

`--json-path` takes a dotted path (`results`, `response.data.items`; a leading
`.` or `$.` is tolerated) and:

- descends the wrapping object to the record array and validates it directly;
- wraps a **single record object** into a one-row table, so single-record
  responses work too;
- errors with **exit 2** and the available keys if the path is wrong
  (`no key \`resultz\` at that level (available keys: count, results, status)`),
  if it lands on a scalar, or if it is passed for non-JSON input.

The extra `serde_json` walk is paid **only when the flag is given**; bare JSON
arrays keep the original single-parse path. Verified end-to-end against the raw
`pokeapi`, `randomuser`, and `usgs` envelopes that previously needed a manual
extraction step.

### 3. External APIs are inherently unstable (operational)

`datausa/population` returned **HTTP 404** during the run — endpoints move,
rename, and rate-limit. This is not a PlexusPact issue but a reminder that any
ingestion built on public APIs needs retry/timeout/schema-drift handling at the
fetch layer, *above* validation. PlexusPact's role is to catch the resulting
schema drift (via the registry `diff` / breaking-change detection) rather than
to fetch.

## Summary

- **Capability:** proven on 11 real APIs spanning flat, numeric-heavy, nested,
  single-object, and object-of-scalars shapes.
- **Scalability:** linear, constant-memory, ~47 MB/s / ~55k rows/s, ~18 µs/row
  marginal — **not a pipeline bottleneck**.
- **Reliability (fixed):** nested columns now produce a clear, actionable
  exit-2 error instead of an opaque internal error, on every code path.
- **Ergonomics (fixed):** `--json-path` extracts object-wrapped record arrays
  inline on both `init` and `check`, so raw API envelopes validate without a
  manual pre-step.
