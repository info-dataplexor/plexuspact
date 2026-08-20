# Contract reference

A contract is a single YAML file (conventionally `contract.yaml`) describing one dataset: its schema, its quality rules, who owns it, and how severely to treat each violation. This page documents every field in the v1 schema. A JSON Schema for editor autocomplete is published at `schema/contract.v1.json` in the repository.

## Full example

```yaml
apiVersion: v1
dataset: user_signups
owner: growth-team@acme.com
description: Daily signup export from the app database.
consumers:
  - { name: analytics-core, contact: data-team@acme.com }

columns:
  user_id:   { type: string, required: true, checks: [unique] }
  email:     { type: string, required: true, pii: email, checks: [{ format: email }] }
  age:       { type: int,    checks: [{ min: 18 }, { max: 120 }] }
  country:   { type: string, checks: [{ length: 2 }, { regex: "^[A-Z]{2}$" }] }
  plan:      { type: string, checks: [{ enum: [free, pro, enterprise] }] }
  signed_up: { type: datetime, required: true }

dataset_checks:
  - row_count_min: 1
  - freshness: { column: signed_up, max_age: 48h, severity: warn }
  - null_ratio_max: { column: age, ratio: 0.10 }

settings:
  allow_extra_columns: true
  on_type_mismatch: error
```

## Top-level fields

| Field | Type | Required | Description |
|---|---|---|---|
| `apiVersion` | string | yes | Contract schema version. Must be `v1`. |
| `dataset` | string | yes | Logical name of the dataset. Together with the contract's content hash, this is the dataset's identity in results and (Phase 2) the registry. |
| `owner` | string | no | Who to talk to when the contract fails — typically a team email. Echoed into every report. |
| `description` | string | no | Human context for consumers reading the contract. |
| `version` | string | no | Free-form version label for the contract itself (e.g. `"2024-11"` or a semver). Metadata only — no engine behavior; changes are classified `cosmetic` by `plexuspact diff`. |
| `consumers` | list | no | Who depends on this dataset. Each entry: `{ name, contact }`. *Reserved field* — see below. |
| `columns` | map | no | Column name → [column definition](#column-definitions). YAML order is preserved in reports. |
| `dataset_checks` | list | no | [Dataset-level checks](#dataset-level-checks) that apply to the whole file, not individual rows. |
| `settings` | map | no | [Global behavior settings](#settings). |

### Reserved fields: `consumers`, `pii`, `classification`

These fields ship in the v1 schema **now** but drive no engine behavior in Phase 1: they are parsed, validated, and echoed into the JSON result and HTML report — nothing more. They exist because contract schemas are painful to change once committed across many repos, and Phase 2 features (blast-radius analysis, compliance evidence) depend on them. Tag PII columns today; get compliance coverage reporting later without touching a single contract. See [ADR-010](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/010-reserved-schema-fields.md).

## Column definitions

Each entry under `columns` maps a column name to:

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | One of `string`, `int`, `float`, `bool`, `date`, `datetime`. The reader is forced to this dtype; values that fail to parse are handled per `settings.on_type_mismatch`. |
| `required` | bool | no (default `false`) | The column must exist **and contain no nulls**. A missing declared column always fails regardless of `required`. |
| `pii` | string | no | *Reserved.* One of `none`, `email`, `phone`, `name`, `address`, `national_id`, `financial`, `health`, `other`. |
| `classification` | string | no | *Reserved.* One of `public`, `internal`, `confidential`, `restricted`. |
| `checks` | list | no | Column-level checks, documented below. |

### Check forms and severity

Checks come in two YAML forms — a **bare** form for parameterless checks and a **map** form for parameterized ones:

```yaml
checks:
  - unique                 # bare form
  - not_empty_string       # bare form
  - { min: 18 }            # map form
  - { min: 18, severity: warn }
```

Every map-form check accepts an optional `severity: error | warn` (default `error`). `error` failures set exit code 1; `warn` failures are reported but exit 0 — unless you run with `--strict`, which promotes warnings to failures. See [exit codes](exit-codes.md).

## The check library

Twenty core checks: 4 schema, 11 value, 5 dataset (plus the `custom_expr` escape hatch counted among them).

### Schema checks

#### 1. `type`

Implicit in every column declaration — values must parse as the declared type.

```yaml
columns:
  age: { type: int }
```

#### 2. `required`

Column exists and has zero nulls.

```yaml
columns:
  user_id: { type: string, required: true }
```

#### 3. `columns_exact`

Dataset must have exactly the declared columns — no extras, none missing. Set in `settings`:

```yaml
settings:
  columns_exact: true
```

#### 4. `allow_extra_columns`

Whether undeclared columns are tolerated (default `true`). `allow_extra_columns: false` is the looser sibling of `columns_exact`: extras fail, but declared-column order doesn't matter.

```yaml
settings:
  allow_extra_columns: false
```

### Value checks (column-level)

#### 5. `unique`

No duplicate values. Exact by default (hash set); for very high-cardinality columns opt into approximate counting with HyperLogLog — see [ADR-005](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/005-exact-vs-approx-unique.md).

```yaml
checks:
  - unique                          # exact (default)
  - { unique: { approx: true } }    # HLL, constant memory
```

Approx mode reports `distinct_estimate` (with `approx: true`) instead of row samples, and only fails when the estimate falls more than 2% below the evaluated row count — ≈2.5σ of the sketch's ±0.81% error, so clean data cannot false-fail. Duplicate counts follow [ADR-009](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/009-duplicate-accounting.md): second-and-later occurrences are the failures.

#### 6. `min`

Values ≥ bound. Numeric or temporal columns.

```yaml
checks: [{ min: 18 }]
```

#### 7. `max`

Values ≤ bound.

```yaml
checks: [{ max: 120 }]
```

#### 8. `regex`

String values fully match the pattern (Rust `regex` syntax — no backtracking, so no pathological patterns).

```yaml
checks: [{ regex: "^[A-Z]{2}$" }]
```

#### 9. `enum`

Values come from a fixed set.

```yaml
checks: [{ enum: [free, pro, enterprise] }]
```

#### 10. `length`

String length — exact, or a range:

```yaml
checks:
  - { length: 2 }                   # exactly 2
  - { length: { min: 1, max: 64 } } # range
```

#### 11. `not_empty_string`

No `""` values (nulls are the business of `required`).

```yaml
checks: [not_empty_string]
```

#### 12–17. `format`

Curated validators for common shapes:

```yaml
checks:
  - { format: email }              # 12
  - { format: uuid }               # 13
  - { format: iso_date }           # 14 — YYYY-MM-DD
  - { format: iso_datetime }       # 15 — ISO-8601 timestamp
  - { format: url }                # 16
  - { format: country_code_iso2 }  # 17 — ISO-3166 alpha-2
```

#### Column-level forms of the ratio and expression checks

`null_ratio_max`, `unique_ratio_min`, and `custom_expr` are documented below under
[dataset-level checks](#dataset-level-checks), but they are **also valid inside a
column's `checks` list**, where they apply to that column implicitly (no `column:`
key needed):

```yaml
columns:
  age:
    type: int
    checks:
      - { null_ratio_max: 0.10 }          # ≤ 10% of age values may be null
  session_id:
    type: string
    checks:
      - { unique_ratio_min: 0.95 }        # ≥ 95% distinct
      - { custom_expr: "col('age') >= 0", severity: warn }
```

### Dataset-level checks

Listed under `dataset_checks`. These never apply per-row; they gate the run as a whole.

#### 18a. `row_count_min` / 18b. `row_count_max`

```yaml
dataset_checks:
  - row_count_min: 1
  - row_count_max: 10000000
```

#### 19. `freshness`

The newest value in a timestamp column is at most `max_age` old. Durations use humantime syntax: `30m`, `48h`, `7d`.

```yaml
dataset_checks:
  - freshness: { column: signed_up, max_age: 48h, severity: warn }
```

#### 20a. `null_ratio_max` / 20b. `unique_ratio_min`

Bounds on a column's null ratio / distinct ratio across the whole dataset:

```yaml
dataset_checks:
  - null_ratio_max: { column: age, ratio: 0.10 }
  - unique_ratio_min: { column: session_id, ratio: 0.95 }
```

### Escape hatch: `custom_expr`

A [Polars expression](https://docs.pola.rs/) that must evaluate to true per row. Expressions only — no arbitrary code, no filesystem access. Use it when no built-in check fits; if you reach for it often, file a feature request.

```yaml
columns:
  discount:
    type: float
    checks:
      - { custom_expr: "col('discount') <= col('price') * 0.5", severity: warn }
```

## Settings

| Setting | Type | Default | Description |
|---|---|---|---|
| `allow_extra_columns` | bool | `true` | Tolerate columns not declared in the contract. |
| `columns_exact` | bool | `false` | Require exactly the declared columns (overrides `allow_extra_columns`). |
| `on_type_mismatch` | `error` \| `warn` | `error` | Severity for values that fail to parse as the declared type. A mismatch is a reported check failure with sample rows — never a crash. |

## Contract discovery

Pass a contract explicitly with `--contract path.yaml`. Without the flag, `plexuspact` looks for `contract.yaml` in the current directory, then for `.plexuspact/*.yaml` (the directory form is canonical when a repo has multiple datasets — one file per dataset).

## Validating and evolving contracts

- `plexuspact validate-contract contract.yaml` lints the file: unknown check names, invalid regexes, `min` > `max`, duplicate columns — with line/column-pointed errors.
- `plexuspact diff old.yaml new.yaml` classifies every change as **breaking** (column removed, type narrowed, new `required`, check tightened), **non-breaking** (optional column added, check loosened, new `warn` check), or **cosmetic** (description/owner edits), and exits 1 on breaking changes — wire it into the CI of the repo that owns the contract.
