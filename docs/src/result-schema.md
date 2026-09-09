# JSON result schema

`plexuspact check --format json` emits a single JSON document called the **RunResult**. It is the single source of truth for a run: the human output, the JUnit XML, the HTML report, and the GitHub Action's PR annotations are all pure renderings of this same document. Nothing is computed by a renderer.

## Versioning policy

The document carries `result_schema_version`. Within version `1`, changes are **additive only**: new fields may appear, but existing fields are never renamed, removed, or repurposed. Consumers must ignore unknown fields. A field's meaning changing would require `result_schema_version: 2`. See [ADR-007](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/007-result-schema-versioning.md).

## Example

```json
{
  "result_schema_version": 1,
  "tool_version": "0.1.0",
  "contract": {
    "dataset": "user_signups",
    "path": "contract.yaml",
    "content_sha256": "d1f2…"
  },
  "source": {
    "path": "data.csv",
    "format": "csv",
    "rows": 1204441,
    "columns": 6,
    "bytes": 812345678
  },
  "started_at": "2026-07-09T10:11:12Z",
  "duration_ms": 6231,
  "status": "failed",
  "summary": { "checks_total": 14, "passed": 12, "failed_error": 1, "failed_warn": 1 },
  "checks": [
    {
      "id": "email.format.email",
      "column": "email",
      "kind": "format",
      "params": { "format": "email" },
      "severity": "error",
      "status": "failed",
      "metrics": { "rows_evaluated": 1204441, "rows_failed": 3, "fail_ratio": 2.49e-6 },
      "samples": [ { "row": 10442, "value": "bob@@example" } ]
    }
  ]
}
```

## Field reference

### Top level

| Field | Type | Description |
|---|---|---|
| `result_schema_version` | int | Schema version of this document; currently `1`. |
| `tool_version` | string | The `plexuspact` version that produced the run. |
| `contract` | object | Identity of the contract used. |
| `source` | object | Identity and shape of the validated input. |
| `started_at` | string | RFC 3339 UTC timestamp of run start. |
| `duration_ms` | int | Wall-clock duration. |
| `status` | string | `passed` \| `failed` — `failed` iff any **error**-severity check failed. This field is independent of `--strict`: `--strict` only affects the process **exit code** (warn-only failures still exit non-zero under `--strict`), never the recorded `status`. |
| `summary` | object | `checks_total`, `passed`, `failed_error`, `failed_warn`. |
| `checks` | array | One entry per planned check, **passing checks included**. |
| `observed_schema` | object | The shape the source actually had: `typed` and `columns[] { name, dtype }` in source order. Absent from older results. |
| `profile` | object | What every column looked like — see below. Absent under `--no-profile` and from older results. |

### `contract`

| Field | Description |
|---|---|
| `dataset` | The contract's `dataset` name. |
| `path` | Contract file path as given on the command line. |
| `content_sha256` | SHA-256 of the contract file's bytes. `dataset` + this hash is a run's contract identity — computable offline, no server-issued IDs. |

### `source`

| Field | Description |
|---|---|
| `path` | Input path, or `-` for stdin. |
| `format` | `csv` \| `parquet` \| `ndjson` \| `json`. |
| `rows`, `columns`, `bytes` | Observed dataset shape. `bytes` is the compressed on-disk size where applicable. |

### `checks[]`

| Field | Description |
|---|---|
| `id` | Stable identifier: `<column>.<kind>[.<param>]` for column checks, `dataset.<kind>` for dataset checks. Stable across runs of the same contract — key drift tracking on it. |
| `column` | Column name; `null` for dataset-level checks. |
| `kind` | Check kind (`format`, `min`, `unique`, `row_count_min`, …). |
| `params` | The check's parameters exactly as configured. |
| `severity` | `error` \| `warn`. |
| `status` | `passed` \| `failed`. |
| `metrics` | Row-level checks report `rows_evaluated`, `rows_failed`, and `fail_ratio`; these are **omitted** for pure dataset-level checks (e.g. `row_count_min`, `freshness`) where a per-row count is meaningless, so consumers must treat them as optional (`jq` returns `null`). Checks also report observed statistics (e.g. `min_observed`, `null_ratio`, `distinct_count`) **even when passing** — this is what makes time-series drift charts possible from stored results. |
| `samples` | Up to `--sample-failures` (default 5) `{ row, value }` pairs. `row` is the absolute row number in the source. Sample values are real data — use `--redact-samples` to mask them before sharing. |

### `profile`

Recorded on every run unless `--no-profile` is given. The checks say whether the data kept the contract's promises; this says what the data *was*, so the next run can be held against it. A feed whose every check passes can still halve its row count, double its nulls, or grow a new category overnight, and the only way to see that is to have written down what yesterday looked like. PlexusPact Cloud compares consecutive runs' profiles and raises those as drift.

| Field | Description |
|---|---|
| `columns[]` | One entry per **source** column, declared or not, in source order. |
| `columns[].name` | Column name as the source spells it. |
| `columns[].null_count`, `null_ratio` | Nulls and empty strings; the ratio is over `source.rows`. |
| `columns[].distinct` | Estimated distinct non-null values (HyperLogLog, about 1% error, 16 KiB per column however wide the data). |
| `columns[].min`, `max` | Bounds, rendered: numeric when every value was a number, otherwise lexical. |
| `columns[].mean`, `std` | Present when every non-null value was a number. |
| `columns[].min_length`, `max_length` | Shortest and longest value, in characters. |
| `columns[].values` | The sorted distinct values, when there were at most 25 and none longer than 64 characters — the categories of a `status`/`plan`/`region` column. Empty means *not collected*, never *no values*. |
| `columns[].values_complete` | Whether `values` is the whole set. |
| `redacted` | `true` under `--redact-samples`: `min`, `max` and `values` are dropped, because a bound is a row's value and a category list is a list of them. Counts, ratios, means and lengths stay. |

## Consuming it

- `jq` one-liners: `jq -r '.checks[] | select(.status=="failed") | .id' run.json`
- The [GitHub Action](ci-recipes.md#github-actions) turns each failed check into a PR annotation this way.
- Phase 2's `plexuspact push` sends exactly this document to the hosted registry (minus `samples` unless opted in — sample values are customer data).
