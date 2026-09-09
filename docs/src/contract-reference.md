# Contract reference

A contract is a single YAML file (conventionally `contract.yaml`) describing one dataset: its schema, its quality rules, who owns it, and how severely to treat each violation. This page documents every field in the v1 schema. A JSON Schema for editor autocomplete is published at `schema/contract.v1.json` in the repository.

## Full example

```yaml
apiVersion: v1
dataset: user_signups
owner: growth-team@acme.com
description: Daily signup export from the app database.
consumers:
  - { name: analytics-core, contact: data-team@acme.com, reads: [user_id, plan] }
  - { name: finance-close, contact: finance@acme.com }

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
| `consumers` | list | no | Who depends on this dataset. Each entry: `{ name, contact, reads }` — see [consumers](#consumers). |
| `columns` | map | no | Column name → [column definition](#column-definitions). YAML order is preserved in reports. |
| `dataset_checks` | list | no | [Dataset-level checks](#dataset-level-checks) that apply to the whole file, not individual rows. |
| `migration` | map | no | The grace period for the breaking changes this version introduces: `{ window_ends, note }` — see [the migration window](#the-migration-window). |
| `settings` | map | no | [Global behavior settings](#settings). |

### Consumers

`consumers` names who inside your company depends on this dataset. Each entry takes:

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The team, service or job that reads this dataset. |
| `contact` | string | no | Who to tell when it breaks — typically a team email or channel. |
| `reads` | list of strings | no | The columns this consumer actually depends on. Every name must be a declared column; an unknown one fails validation, because a typo here would silently narrow the answer to *who breaks*. |

```yaml
consumers:
  - { name: analytics-core, contact: data-team@acme.com, reads: [user_id, plan] }
  - { name: finance-close, contact: finance@acme.com }
```

Omitting `reads` is not the same as listing every column: it means this consumer's dependency has never been narrowed, so anything that moves may reach it. Tools report that as an undeclared dependency rather than a measured one.

The validation engine never reads any of this — a consumer changes no verdict, and `plexuspact check` behaves identically with the list present or absent. What it drives is *blast radius*: given a change or a failing column, which consumers are downstream of it and who to contact. `plexuspact diff` classifies edits to the list as cosmetic (they move no data), and the list is echoed into the JSON result and HTML report. PlexusPact Cloud reads it to answer "who breaks" on the review screen, on a failing run, and in a `contract.changed` alert.

### Reserved fields: `pii`, `classification`

These two ship in the v1 schema **now** but drive no engine behavior: they are parsed, validated, and echoed into the JSON result and HTML report — nothing more. They exist because contract schemas are painful to change once committed across many repos, and compliance-evidence reporting depends on them. Tag PII columns today; get coverage reporting later without touching a single contract. See [ADR-010](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/010-reserved-schema-fields.md).

## Column definitions

Each entry under `columns` maps a column name to:

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | One of `string`, `int`, `float`, `bool`, `date`, `datetime`. The reader is forced to this dtype; values that fail to parse are handled per `settings.on_type_mismatch`. |
| `description` | string | no | What the column means. No engine behavior — and the only place a consumer can find out whether they are reading it correctly, which is why *changing* one is classified `semantic` rather than cosmetic. |
| `required` | bool | no (default `false`) | The column must exist **and contain no nulls**. A missing declared column always fails regardless of `required`. |
| `pii` | string | no | *Reserved.* One of `none`, `email`, `phone`, `name`, `address`, `national_id`, `financial`, `health`, `other`. |
| `classification` | string | no | *Reserved.* One of `public`, `internal`, `confidential`, `restricted`. |
| `stability` | string | no (default `stable`) | The promise attached to this column: `stable`, `beta`, or `deprecated`. See [announcing a retirement](#announcing-a-retirement). |
| `sunset` | string | no | The date (`YYYY-MM-DD`) after which the column may be removed. Pairs with `stability: deprecated`. |
| `checks` | list | no | Column-level checks, documented below. |

### Announcing a retirement

Removing a column is breaking whichever way you do it. The difference between a
breaking change and an outage is whether the people downstream were told early
enough to do something about it, and these two fields are how a contract says it
in advance:

```yaml
columns:
  amount:
    type: float
    stability: deprecated
    sunset: 2026-12-31
    description: gross amount — read `amount_minor` instead (minor units, net of refunds)
```

`stability` is the promise; `sunset` is the date the promise runs out. They only
work as a pair: a `deprecated` column with no date tells a consumer to move
without saying by when, and a `sunset` on a column still marked `stable` says
the opposite of what the date says. Either half on its own validates with a
warning, never an error — half an announcement is still more than most contracts
say today, and rejecting it would only mean the supplier says nothing instead.

Dates are plain ISO-8601 strings (`YYYY-MM-DD`). A value that is not a real
calendar date is an error.

`plexuspact diff` reads both as promises rather than metadata:

| Edit | Impact |
|---|---|
| `stable` → `beta` or `deprecated` | `semantic` — no check behaves differently, but the promise is weaker |
| `deprecated` → `stable` | `non_breaking` |
| adding a `sunset` | `semantic` |
| moving a `sunset` **later** | `non_breaking` |
| moving a `sunset` **earlier** | `breaking` — this is not a weaker promise, it is a shorter deadline, and work scheduled against the old date is now late |
| removing a column that was announced | `breaking`, and the diff says which date it was announced for |

### The migration window

Where `sunset` belongs to one column, `migration` describes the version as a
whole: the grace period for the breaking changes this version introduces.

```yaml
migration:
  window_ends: 2026-12-31
  note: read `amount_minor` (minor units); `amount` is removed after the window
```

`window_ends` is required and must be a date. The `note` is what a consumer
actually has to *do*, and leaving it out is a warning — a deadline with no
instructions moves the work downstream without saying what the work is.

The block describes the version it is attached to and is expected to be dropped
once the window closes, so removing it is `cosmetic`; adding one is
`non_breaking`; and bringing `window_ends` forward is `breaking`, for the same
reason moving a `sunset` forward is.

PlexusPact Cloud can enforce it. A project with **require a migration window**
turned on rejects a new version whose diff contains breaking changes unless
`migration.window_ends` is present and still in the future.

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

Twenty-three core checks: 4 schema, 11 value, 8 dataset (the `custom_expr` escape hatch counted among them).

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

No duplicate values. Exact by default (hash set); for very high-cardinality columns opt into approximate counting with HyperLogLog — see [ADR-005](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/005-exact-vs-approx-unique.md).

```yaml
checks:
  - unique                          # exact (default)
  - { unique: { approx: true } }    # HLL, constant memory
```

Approx mode reports `distinct_estimate` (with `approx: true`) instead of row samples, and only fails when the estimate falls more than 2% below the evaluated row count — ≈2.5σ of the sketch's ±0.81% error, so clean data cannot false-fail. Duplicate counts follow [ADR-009](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/009-duplicate-accounting.md): second-and-later occurrences are the failures.

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

#### 21. `primary_key`

Declared at the top level, next to `columns`, because it is a fact about the dataset rather than one more rule: the column (or columns) that identify a row. Every row must carry the whole key, and no key may repeat. A row with a null in any key column fails, as does the second appearance of a key that has been seen already.

```yaml
primary_key: [order_id]          # or [region, order_id] for a composite key
```

The check reports as `dataset.primary_key`, with the number of distinct keys, repeated keys and null-key rows in `observed`. Declaring a key is a breaking change for `diff`: consumers can now rely on it. A `unique` check on the same column is still allowed but redundant.

The keys a passing delivery carried are what another contract's `references` check looks up (below).

#### 22. `assert`

One SQL expression about the whole dataset, evaluated once after every row has been read. It must reduce to a single true or false — a total, a count, a comparison of two aggregates. A per-row rule belongs in `custom_expr`.

```yaml
dataset_checks:
  - assert: "SUM(amount) BETWEEN 990000 AND 1010000"
  - assert: { expr: "COUNT(DISTINCT customer_id) >= 1000", severity: warn }
  - assert: "SUM(CASE WHEN status = 'refunded' THEN 1 ELSE 0 END) * 20 < COUNT(*)"
```

Columns are read as the type the contract declares, so `SUM(amount)` adds numbers whether the file was CSV or Parquet. The expression sees only the columns it names, which is what the engine keeps in memory across batches; memory grows with the width of the assertion, not of the file. An assertion that comes out null (an average of no rows, a column that could not be read as its declared type; a sum of no rows is 0) fails, because unknown never passes.

#### 23. `references`

Every complete key in `columns` must exist in another dataset — the foreign key of a contract. The other dataset is named by its contract's `dataset`; `to` names its key columns and defaults to the same names as `columns`. Rows with a null in any of the columns are not evaluated, as in SQL.

```yaml
dataset_checks:
  - references: { columns: [customer_id], dataset: customers }
  - references: { columns: [region, product_code], dataset: catalog, to: [region, sku], severity: warn }
```

Where the other dataset's keys come from:

- On the command line, `--reference customers=customers.csv` reads them from a file: the `to` columns are collected, hashed, and never held as rows.
- In PlexusPact Cloud, from the last passing run of the `customers` contract, which keeps the key set its `primary_key` produced. `to` must match that contract's `primary_key`.

A `references` check with no key set to consult reports that it could not run, and fails: a reference nobody could look up is not one that held.

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
| `input` | object | — | How the data file is read; see below. |

### Input settings

A feed that is not a plain CSV needs reading instructions: which sheet, which
XML element is a record, where each fixed-width column starts. They belong in
the contract, not in a CI script, so every place that checks the feed reads it
the same way. `plexuspact init` writes them from its flags; `plexuspact check`
applies them, and its flags (`--sheet`, `--skip-rows`, `--xml-record`,
`--fixed-width`, `--delimiter`, `--no-header`, `--input-format`, `--json-path`)
override them for one run.

```yaml
settings:
  input:
    format: excel        # csv | tsv | parquet | ndjson | json | excel | xml | fixed_width
    sheet: Report        # workbooks: name or 1-based position (default: first sheet)
    skip_rows: 2         # rows above the header (report titles)
    has_header: true     # false: the first row is data, columns are named by position
```

| Field | Applies to | Description |
|---|---|---|
| `format` | all | Pins the format instead of going by the file extension. |
| `delimiter` | csv | One character, or `\t`. |
| `has_header` | csv, excel, fixed_width | Whether the first row names the columns. |
| `skip_rows` | csv, excel, fixed_width | Lines to skip before the header. |
| `json_path` | json | Dotted path to the record array inside an envelope (`data.items`). |
| `sheet` | excel | Sheet name or 1-based position. |
| `xml_record` | xml | The element that is one record: a name (`order`) or a path (`orders/order`). Default: the first element under the root. |
| `fixed_width` | fixed_width | The layout: a list of `{ name, start, end }` (1-based, inclusive, in characters) or `{ name, width }` fields laid end to end. |

Workbook cells arrive as text the way a CSV export would show them: dates as
`2026-07-01`, timestamps as `2026-07-01T09:30:00`, booleans as `true`/`false`,
whole numbers without a `.0`. XML attributes and child elements both become
columns; a nested element becomes a dotted column (`address.city`), an
attribute on a child too (`amount.currency`); an empty element or
`xsi:nil="true"` is a null.

```yaml
settings:
  input:
    format: fixed_width
    skip_rows: 2
    has_header: true
    fixed_width:
      - { name: id, start: 1, end: 4 }
      - { name: customer, start: 5, end: 20 }
      - { name: amount, width: 8 }
```

## Contract discovery

Pass a contract explicitly with `--contract path.yaml`. Without the flag, `plexuspact` looks for `contract.yaml` in the current directory, then for `.plexuspact/*.yaml` (the directory form is canonical when a repo has multiple datasets — one file per dataset).

## Validating and evolving contracts

- `plexuspact validate-contract contract.yaml` lints the file: unknown check names, invalid regexes, `min` > `max`, duplicate columns — with line/column-pointed errors.
- `plexuspact diff old.yaml new.yaml` classifies every change as **breaking** (column removed, type narrowed, new `required`, check tightened, a deadline brought forward), **semantic** (a definition rewritten, a promise weakened — nothing fails, but everyone downstream is now working from the old meaning), **non-breaking** (optional column added, check loosened, new `warn` check), or **cosmetic** (owner and version edits), and exits 1 on breaking changes — wire it into the CI of the repo that owns the contract.
