# Checking a warehouse table in place

`plexuspact check` reads rows. That is the right shape for a file: the rows
are already on their way to you, and the engine folds them in one pass however
many there are. A warehouse table is a different thing. Its rows are next to a
query engine that can count, group and compare them far faster than they could
be moved, and moving them is what costs — egress, a pull that times out at five
hundred thousand rows, a copy of a fact table sitting on a validation host.

So for a warehouse the contract goes to the data. The `pushdown` module in
`plexuspact-engine` compiles a contract into SQL the warehouse runs itself, and
decodes what comes back into the same result a pull would have produced —
same check ids, same parameters, same observed statistics, same messages.
PlexusPact Cloud uses it for every Postgres, Redshift, Snowflake, Databricks and
BigQuery source unless the source is set to pull instead; the module is public
API, so a host of your own can do the same.

## What runs

One aggregate scan. For a contract like

```yaml
columns:
  order_id: { type: int, required: true }
  email:
    type: string
    checks:
      - format: email
      - unique: true
  amount:
    type: float
    checks:
      - min: 0
primary_key: [order_id]
dataset_checks:
  - freshness: { column: created_at, max_age: 48h }
```

the warehouse is asked, once, for the row count, `COUNT("order_id")`, the
number of null ids, the number of emails that do not match the address
pattern, the distinct email count, the number of amounts below zero, the
smallest amount, the distinct key count, and the newest `created_at` — every
value cast to text so the answer reads the same from any driver. A profile of
every column (null count, distinct count, bounds, mean and deviation, value
lengths) rides in the same statement, so the warehouse table gets the same
drift record a file does.

`custom_expr` and `assert` checks each run as a statement of their own. A
contract expression the warehouse cannot parse fails that one check with the
warehouse's own error, and every other check still stands.

A second, smaller round runs only when the first says it should: the failing
values of every check that failed (up to `--sample-failures` of them), the
duplicated keys, the value set of a column that turned out to have few values,
and the key set the `primary_key` and `references` checks need.

## What is the same, and what is not

The checks mean the same thing. `required` counts nulls; value checks skip
nulls and count the non-null values that do not match; `unique` reports how
many values are repeated; `null_ratio_max` and `unique_ratio_min` divide the
same counts; `freshness` measures from the newest row to the clock the run was
given; `primary_key` counts repeated keys and rows with a null in the key.
A column declared as a number but stored as text is parsed with the dialect's
safe cast (`TRY_CAST`, `SAFE_CAST`, and on Postgres, which has none, a cast
guarded by a numeric-shape pattern), the way the engine parses a CSV column.

Some things differ, and the result says so rather than papering over them:

- **Failure samples have no row number.** A warehouse has no row order, so
  every sample is reported at row `0`. The value is still the value.
- **`unique` is exact.** `approx: true` is honoured by the pull engine's
  sketch; the warehouse counts distinct values exactly whichever you asked for.
- **`format: iso_date` and `iso_datetime` check the shape, not the calendar.**
  `2026-02-30` passes in the warehouse and fails by pull.
- **A `min`/`max` on a column the warehouse cannot compare numerically** — a
  JSON or binary column, say — reports that the check could not run, as does a
  `freshness` on a Postgres text column (no safe timestamp cast). A check that
  cannot run is a failure, never a quiet pass.
- **`references` looks the foreign keys up on the validation side.** The
  warehouse returns the distinct referring keys with their row counts (up to
  the key cap, 500 000 by default); the ones the referenced dataset does not
  have are counted as failing rows. Above the cap the check reports that the
  feed should be evaluated by pull.
- **Column types are the warehouse's own.** `observed_schema` carries
  `bigint`, `NUMBER(38,0)`, `STRING` as the warehouse names them, and the
  `type` check compares families: a `string` column matches any character
  type, `int` matches any whole-number type, `float` matches either, `date`,
  `datetime` and `bool` their own.

## Using the module

```rust
use plexuspact_engine::pushdown::{compile, Dialect, ProbeOptions, Reply, SourceColumn};

let columns = vec![SourceColumn { name: "order_id".into(), dtype: "bigint".into() } /* … */];
let mut probe = compile(&contract, "\"public\".\"orders\"", Dialect::Postgres, &columns,
                        &ProbeOptions { now: chrono::Utc::now(), ..ProbeOptions::default() });

let first: Vec<Reply> = probe.statements().iter().map(|s| run_sql(&s.sql)).collect();
let second: Vec<Reply> = probe.follow_ups(&first).iter().map(|s| run_sql(&s.sql)).collect();
let engine_out = probe.decode(&first, &second);
let (result, artifacts) = plexuspact_core::assemble(engine_out, /* Assembly { … } */);
```

`run_sql` is yours: it returns `Reply::Rows` with every cell as `Option<String>`,
or `Reply::Error` with the warehouse's message. The table name is passed
already quoted and qualified, so the identifier rules of the warehouse stay
with the caller that knows them (Snowflake upper-cases an unquoted name;
BigQuery quotes the whole `project.dataset.table` path as one identifier).
`plexuspact_core::assemble` turns the engine output into the same `RunResult`
`plexuspact check` writes, so a warehouse run and a file run compare and report
alike.
