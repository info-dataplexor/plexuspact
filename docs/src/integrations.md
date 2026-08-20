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
transform ships in the OSS CLI; anything that holds credentials, retries, or
persistent configuration lives in the proprietary control plane.**

| Integration | Home | Why |
|---|---|---|
| Exit codes + JUnit XML + GitHub Action | **OSS** | Offline, deterministic; the CI gate is the core promise |
| **OpenLineage event emission** | **OSS** | Pure `RunResult → JSON` transform, no secrets |
| dbt source/test generation | **OSS** | Pure `contract → YAML` transform |
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
- **dbt** — run PlexusPact as a pre-`dbt run` gate in CI (below) so a broken
  contract fails the build before models compute; feed the OpenLineage event into
  the same collector `dbt-ol` reports to for a unified lineage view. (A **dbt
  source/test generator** — `contract → schema.yml` with tests — is on the
  roadmap.)
- **Airflow / Marquez / DataHub** — consume the OpenLineage event directly.

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
2. **dbt adapter (OSS + cloud)** — generate `schema.yml` sources/tests from a
   contract; optionally publish results into the dbt artifacts a run consumes.
3. **Databricks Delta audit sink (cloud)** — the cloud complement to the DLT
   generator above: write every run's summary to a Delta table for history.
4. **Generic outbound webhook (cloud)** — POST the `RunResult` (or a compact
   summary) to any endpoint, for teams with their own tooling.
5. **Warehouse audit sink (cloud)** — append every run's summary to a Snowflake /
   BigQuery / Postgres table for long-horizon quality dashboards.
