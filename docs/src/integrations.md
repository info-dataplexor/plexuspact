# Integrations: communicating quality to the tools that consume the data

PlexusPact's job is to decide, at the source, whether a dataset honors its
contract. That verdict is only useful if it reaches the systems that act on it —
the transformation tools, orchestrators, catalogs, and humans downstream. This
page describes how quality signals leave PlexusPact and where each integration
lives in the open-core split.

## One signal, many consumers

Every `plexuspact check` run produces exactly one machine-readable artifact: the
versioned [`RunResult`](result-schema.md) document. **Every integration is a
pure projection of that document** — no integration re-computes a verdict, so all
of them agree by construction. This is the same guarantee the report renderers
already hold (`human`/`json`/`junit`/`html`), extended outward.

```
                         ┌───────────────────────────── RunResult (v1) ─────────────────────────────┐
 data + contract ─▶ check ─▶ { status, summary, checks[], source, contract }
                         └───────────────────────────────────┬──────────────────────────────────────┘
                                                              │
        ┌───────────────────────┬─────────────────────────┬──┴───────────────────┬─────────────────────────┐
        ▼                       ▼                         ▼                       ▼                         ▼
   exit code + JUnit       OpenLineage event         warehouse table         ChatOps message          catalog / lineage
   (CI gate)               (Databricks/dbt/Airflow)  (audit history)         (Slack/Teams/PagerDuty)  (Marquez/DataHub)
```

## Where each integration lives (open-core boundary)

The boundary follows one rule: **anything that is a pure, offline format
transform ships in the OSS CLI; anything that *stores* credentials, retries, or
persistent configuration lives in the proprietary control plane.**

One integration sits deliberately on the line and is worth naming: the CLI can
[report its own result](cloud.md) to a cloud project. It reads a single token
from the environment, sends to a single address, stores nothing, and retries
nothing — that is what keeps it on the OSS side. The moment a signal needs to be
*routed* — several destinations, stored webhook secrets, backoff, per-workspace
rules — it belongs to the control plane.

| Integration | Home | Why |
|---|---|---|
| Exit codes + JUnit XML + GitHub Action | **OSS** | Offline, deterministic; the CI gate is the core promise |
| **Reporting a run to a cloud project** | **OSS** | One token from the environment, one address, nothing stored; opt-in and suppressible with `--offline` |
| **OpenLineage event emission** | **OSS** | Pure `RunResult → JSON` transform, no secrets |
| **dbt `schema.yml` generation + test package** | **OSS** | Pure `contract → YAML` transform; the tests are dbt macros |
| **Databricks DLT expectation generation** | **OSS** | Pure `contract → SQL/Python` transform |
| Delivering events to a collector URL | **Cloud** | Holds endpoint + token, needs retry/backoff |
| Slack / Teams / PagerDuty notifications | **Cloud** | Stores webhook secrets, per-workspace routing |
| Warehouse audit table writes | **Cloud** | Holds warehouse credentials, manages schema |
| Notification rules (who, when, threshold) | **Cloud** | Persistent, per-workspace configuration |

This keeps the OSS binary dependency-light and airgap-friendly while giving the
SaaS a natural, defensible surface: the stateful, credential-bearing *routing*
of quality signals.

## OpenLineage (available now)

[OpenLineage](https://openlineage.io/) is the open standard for run/quality
events. Emitting it means **one integration reaches many consumers** — Databricks
(native OpenLineage ingestion), dbt (via `dbt-ol`), Airflow, Marquez, and DataHub
all speak it — instead of a bespoke adapter per tool. This is the recommended
default path for "tell the tools that consume the data."

```bash
plexuspact check orders.parquet \
  --contract orders.contract.yaml \
  --openlineage run-event.json \
  --openlineage-namespace 's3://lake'      # storage system the dataset lives in
```

The emitted `RunEvent` carries the standard data-quality facets, mapped purely
from the `RunResult`:

- **`dataQualityAssertions`** (dataset facet) — one assertion per check:
  `{ "assertion": "email.format.email", "success": false, "column": "email" }`.
- **`dataQualityMetrics`** (input-dataset facet) — `rowCount`, `bytes`, and
  per-column `nullCount` / `distinctCount` / `count` derived from check metrics.
- **`plexuspact_contract`** (run facet) — contract dataset, content hash, tool
  version, and the run summary, so a consumer can trace any quality event back to
  an exact contract version.
- **`ownership`** (job facet) — the contract owner, when declared.

The event's `eventType` is `COMPLETE` on a passing run and **`FAIL` when any
error-severity check failed**, so an orchestrator can gate on the event itself.
The `runId` is a deterministic UUID derived from the contract hash and start
time, so re-emitting the same run is idempotent.

### Reaching the specific tools

- **Databricks** — two complementary paths:
  1. **Lineage/quality view** — point a Databricks job's OpenLineage endpoint at
     your collector, or forward the emitted event; the assertions surface against
     the dataset in Unity Catalog lineage.
  2. **Checks inside the pipeline** — generate Delta Live Tables expectations
     from the contract so the *same* rules run natively in DLT (see below).
- **dbt** — generate the model's (or source's) `schema.yml` from the contract so
  the *same* checks run as dbt tests (see below); run PlexusPact as a
  pre-`dbt run` gate in CI so a broken partner feed fails before models
  compute; feed the OpenLineage event into the same collector `dbt-ol` reports
  to for a unified lineage view.
- **Airflow / Marquez / DataHub** — consume the OpenLineage event directly.

## dbt (available now)

Generate a dbt `schema.yml` from a contract, so the rules PlexusPact enforces at
the door also run as `dbt test` on the model — one source of truth, no
double-authoring:

```bash
# the dataset as a model
plexuspact export orders.contract.yaml --target dbt --out models/orders.yml

# the dataset as a table of a source, with native `dbt source freshness`
plexuspact export orders.contract.yaml --target dbt --source raw --out models/staging/raw.yml
```

Where dbt has a native test it is used — `required` → `not_null`, `unique` →
`unique`, `enum` → `accepted_values`, single-column `references` →
`relationships`, `freshness` on a source → `loaded_at_field` + `freshness:`.
Every other check (`min`, `max`, `regex`, `format`, `length`,
`not_empty_string`, `null_ratio_max`, `unique_ratio_min`, `row_count_min/max`,
`assert`, model-level `freshness`) is a `plexuspact.*` generic test from the
[PlexusPact dbt package](https://github.com/info-dataplexor/plexuspact/tree/main/integrations/dbt),
whose semantics match `plexuspact check` (value checks ignore nulls; nullability
is `required`, never a side effect). Add it once:

```yaml
# packages.yml
packages:
  - git: "https://github.com/info-dataplexor/plexuspact.git"
    subdirectory: integrations/dbt
    revision: v0.1.0
```

Warn-severity checks become `severity: warn`; PII, classification, stability and
sunset land in column `meta`, owner and contract version in the table's. What
cannot be a dbt test — composite `references`, `custom_expr` — is listed in a
trailing comment so nothing is dropped silently. Regular expressions dispatch
per adapter (Postgres, Redshift, Snowflake, BigQuery, Databricks/Spark, DuckDB).
The generated file uses `data_tests` with inputs under `arguments`, the shape
dbt has used since 1.10; on 1.8/1.9 unnest `arguments`.

## Databricks Delta Live Tables (available now)

Generate DLT expectations directly from a contract, so the rules PlexusPact
enforces at the source also run *inside* the DLT pipeline — no double-authoring:

```bash
# SQL CONSTRAINT … EXPECT (…) clauses (default)
plexuspact export orders.contract.yaml --target databricks-dlt

# Python @dlt.expect_all* decorators
plexuspact export orders.contract.yaml --target databricks-dlt --lang python --out orders_dlt.py
```

The mapping is faithful to PlexusPact's semantics:

- `required` → `` `col` IS NOT NULL ``; value checks permit null
  (`col IS NULL OR …`) since nullability is governed by `required`, mirroring the
  engine.
- `min` / `max` / `length` / `enum` / `regex` / `format` → row-level Spark-SQL
  predicates (formats expand to backslash-free POSIX regexes that survive Spark
  literal escaping).
- **Error**-severity checks emit `ON VIOLATION FAIL UPDATE` (a violation stops
  the DLT update, matching PlexusPact's exit 1); **warn**-severity checks are
  recorded without failing.
- Checks that are inherently aggregate (`unique`, `null_ratio_max`,
  `unique_ratio_min`, `row_count_*`) or engine-specific (`custom_expr`, a Polars
  expression) **cannot** be per-row DLT expectations. They are listed in a
  trailing comment — never silently dropped — with a note to keep enforcing them
  via `plexuspact check` upstream.

The command validates the contract first and exits `2` on a broken contract, so
you never generate expectations from a contract that wouldn't itself run. A
Delta **audit-table sink** (run history for dashboards) is the cloud-side
complement on the roadmap.

## Open Data Contract Standard (available now)

PlexusPact reads and writes [ODCS](https://bitol-io.github.io/open-data-contract-standard/)
v3 documents, the Linux Foundation (Bitol) standard that catalogs and
governance tools exchange. Three doors:

```bash
# Publish a contract as ODCS 3.1 (for a catalog, a partner, a governance review)
plexuspact export orders.contract.yaml --target odcs --id urn:acme:orders --out orders.odcs.yaml

# Bring a document somebody else wrote in, keep the draft, review it
plexuspact import partner.odcs.yaml --out partner.contract.yaml

# Or skip the conversion step: every command takes an ODCS document where it
# takes a contract, converting on the way in and saying what it dropped
plexuspact check feed.csv --contract partner.odcs.yaml
```

What has a native ODCS slot goes there, so a foreign reader sees a rule and not
a vendor blob:

- Types, `required`, `unique`, `primaryKey` (with position for a composite
  key), `classification`, and error-severity `min` / `max` / `regex` /
  `length` / `format` in `logicalTypeOptions`.
- `references` → a schema `relationships` entry (`type: foreignKey`, ODCS 3.1);
  `freshness` → the `latency` SLA property; a column's `sunset` → its
  `endOfLife`; `row_count_min` / `row_count_max` → the standard `rowCount`
  library metric.
- Everything else (`enum`, ratios, `assert`, `custom_expr`, and every
  warn-severity check, since ODCS options carry no severity) rides in
  spec-sanctioned custom quality entries (`type: custom, engine: plexuspact`),
  which is what makes export → import lossless for the whole check library.

Import is tolerant: an `object` column, a `sql` quality rule, a `retention`
promise have no PlexusPact check, so they are dropped with a note on stderr
rather than failing the conversion. The draft is linted before it is written;
`import` exits `2` when it still has errors, and `0` when it is usable as it
stands. Documents from `v3.0.x` through `v3.2` are accepted; export stamps
`v3.1.0`.

## CI gate (available now)

The original "block bad data at the source" path needs no new integration:

- **Exit codes** — `0` pass, `1` contract violated, `2` user error, `3` internal
  ([exit codes](exit-codes.md)). Any orchestrator step gates on this.
- **JUnit XML** (`--format junit`) — surfaces per-check results in the CI UI.
- **GitHub Action** — annotates the PR with failing checks.

Put `plexuspact check` *before* the transformation step (dbt, Spark, DLT) and a
failing contract stops the pipeline instead of propagating bad data.

## Roadmap

Prioritized, each building on the `RunResult` projection principle:

1. **ChatOps notifications (cloud)** — Slack / Teams / PagerDuty channels
   configured per workspace, firing on failed runs with owner routing. The most
   direct answer to "quality communication" for humans.
2. **dbt results into the cloud** — publish a dbt job's `run_results.json` as a
   run against the registered contract, so a test that fails inside dbt shows on
   the same trust page as a delivery that failed at the door.
3. **Databricks Delta audit sink (cloud)** — the cloud complement to the DLT
   generator above: write every run's summary to a Delta table for history.
4. **Generic outbound webhook (cloud)** — POST the `RunResult` (or a compact
   summary) to any endpoint, for teams with their own tooling.
5. **Warehouse audit sink (cloud)** — append every run's summary to a Snowflake /
   BigQuery / Postgres table for long-horizon quality dashboards.
