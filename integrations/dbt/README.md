# PlexusPact dbt package

Generic tests that give a dbt project the same check library a PlexusPact
contract has. `plexuspact export --target dbt` writes a `schema.yml` whose
tests are either dbt's own (`not_null`, `unique`, `accepted_values`,
`relationships`, source freshness) or `plexuspact.*` tests from this package.

## Install

```yaml
# packages.yml
packages:
  - git: "https://github.com/info-dataplexor/plexuspact.git"
    subdirectory: integrations/dbt
    revision: v0.1.0   # pin to a release tag
```

```bash
dbt deps
plexuspact export contracts/user_signups.yaml --target dbt --source raw > models/staging/user_signups.yml
dbt test
```

The generated file uses `data_tests` and nests test inputs under
`arguments`, the shape dbt has used since 1.10. On dbt 1.8 or 1.9, unnest
the `arguments` block of each test; the package itself works from dbt 1.5.

## Tests

| test | arguments | semantics (same as `plexuspact check`) |
|------|-----------|----------------------------------------|
| `plexuspact.min` | `value` | every non-null value `>= value` |
| `plexuspact.max` | `value` | every non-null value `<= value` |
| `plexuspact.regex` | `pattern` | every non-null value matches (search, not full match) |
| `plexuspact.format` | `format` | `email`, `uuid`, `iso_date`, `iso_datetime`, `url`, `country_code_iso2` |
| `plexuspact.length` | `exact` or `min` / `max` | string length bounds, nulls ignored |
| `plexuspact.not_empty_string` | | no empty strings (nulls are governed by `not_null`) |
| `plexuspact.null_ratio_max` | `ratio` | nulls / rows `<= ratio` |
| `plexuspact.unique_ratio_min` | `ratio` | distinct non-null / non-null `>= ratio` |
| `plexuspact.row_count_min` | `count` | at least `count` rows (table-level) |
| `plexuspact.row_count_max` | `count` | at most `count` rows (table-level) |
| `plexuspact.freshness` | `column`, `max_age_seconds` | `max(column)` within the window (table-level; sources use dbt's own freshness) |
| `plexuspact.assert_expr` | `expression` | an aggregate SQL predicate over the table, e.g. `MIN(amount) >= 0` |

Regular expressions dispatch per adapter: Postgres and Redshift (`~`),
Snowflake (`regexp_instr`), BigQuery (`regexp_contains`), Databricks and
Spark (`rlike`), DuckDB (`regexp_matches`). Anything else gets the
Postgres form; add a `<adapter>__regex_match` macro in your project to
override it.

Value checks ignore nulls, exactly as PlexusPact does: whether a column may
be null is `required` in the contract and `not_null` here, never a side
effect of a value rule.
