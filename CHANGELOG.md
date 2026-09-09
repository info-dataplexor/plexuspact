# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
with 0.x rules: breaking CLI or contract-schema changes bump the minor version
and get a `MIGRATION.md` entry.

## [Unreleased]

### Changed

- Minimum supported Rust is 1.88 (was 1.80): the workbook reader needs a calamine that has moved to quick-xml 0.41, which closes RUSTSEC-2026-0194 and RUSTSEC-2026-0195 for attacker-supplied XML.

### Fixed

- Release builds for `x86_64-unknown-linux-musl` and `x86_64-apple-darwin` failed with "can't find crate for `core`": the toolchain pinned by `rust-toolchain.toml` overrides the `stable` the workflow had added cross targets to. The release workflow now adds the target to the pinned toolchain, and can be run by hand (`workflow_dispatch`) to prove the whole matrix before a tag is cut.

### Added

- `plexuspact export --target dbt` writes a dbt `schema.yml` for the contract's dataset: a `models:` entry, or with `--source NAME` a table of that `sources:` entry with native source freshness. `required`, `unique`, `enum` and single-column `references` become dbt's own tests; every other check becomes a `plexuspact.*` generic test from the new dbt package at `integrations/dbt` (`min`, `max`, `regex`, `format`, `length`, `not_empty_string`, `null_ratio_max`, `unique_ratio_min`, `row_count_min`, `row_count_max`, `freshness`, `assert_expr`), with regular expressions dispatched per adapter (Postgres, Redshift, Snowflake, BigQuery, Databricks/Spark, DuckDB). Warn-severity checks are `severity: warn`; PII, classification, stability and sunset ride in column `meta`; composite `references` and `custom_expr` are listed in a trailing comment rather than dropped. Proven against dbt 1.12 on DuckDB: on the demo fixture, dbt fails exactly the checks `plexuspact check` fails.
- `plexuspact_report::openlineage_event` returns the OpenLineage `RunEvent` as a JSON value (what `render_openlineage` serializes), so a host forwarding runs to a lineage collector can add facets of its own.
- pre-commit hooks in `.pre-commit-hooks.yaml`: `plexuspact-validate-contract` parses and lints every staged contract or ODCS document, `plexuspact-diff` refuses a commit whose contract change breaks consumers (compared with `HEAD`). Both skip YAML that is not a contract, fetch the checksum-verified release matching `rev` once, and honour `PLEXUSPACT_BIN`.
- Open Data Contract Standard (ODCS) v3 in the CLI: `plexuspact export --target odcs` writes an ODCS 3.1 document, `plexuspact import` converts one into a contract (what could not be carried across is listed on stderr, and the draft is linted before it is written), and every command that takes a contract file also takes an ODCS document, converting it on the way in. The bridge itself lives in `plexuspact-contract` (`odcs` module). `references` becomes a schema `relationships` entry, `freshness` the `latency` SLA property, a column's `sunset` its `endOfLife`, `row_count_min`/`row_count_max` the standard `rowCount` library metric; the reverse mappings read those slots back, including `nullValues`/`duplicateValues` rules on a column.
- `primary_key: [..]` at the top of a contract: the columns that identify a row. Every row must carry the whole key and no key may repeat; a null in any key column fails. Reported as `dataset.primary_key` with distinct, repeated and null-key counts. `diff` classifies declaring or changing the key as breaking. The distinct keys a passing run carried are handed back to callers (`RunArtifacts`) so a later delivery of another dataset can be checked against them.
- `assert` dataset check: one SQL expression about the whole dataset (`SUM(amount) BETWEEN 990000 AND 1010000`, `COUNT(DISTINCT customer_id) >= 1000`), evaluated once after every row has been read. Only the columns it names are kept between batches, converted to the types the contract declares. An expression that produces more than one value is refused with a pointer at `custom_expr`; one that comes out null fails.
- `references` dataset check: every complete key in `columns` must exist in another dataset (`references: { columns: [customer_id], dataset: customers }`; `to` when the key columns are named differently over there). `check --reference customers=customers.csv` reads the keys from a file; a service that keeps runs supplies them from the referenced dataset's last passing delivery. A check with no key set to consult fails, saying so.
- Excel and OpenDocument workbooks (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`), XML and fixed-width text as inputs to `check` and `init`, next to CSV/TSV, Parquet, NDJSON and JSON. Workbook cells are read as the text a CSV export would show (dates, timestamps, booleans, whole numbers); XML attributes and children become columns, nested ones dotted (`amount.currency`); fixed-width columns are character spans, so accents do not shift them.
- `settings.input` in the contract carries the reading instructions — `format`, `sheet`, `skip_rows`, `has_header`, `delimiter`, `json_path`, `xml_record`, `fixed_width` — so a partner feed is checked the same way everywhere. `init` records what its flags said; `check` applies the block and lets its flags override it for one run. New flags on both: `--sheet`, `--skip-rows`, `--no-header`, `--delimiter`, `--xml-record`, `--fixed-width`.
- `datetime` columns accept a bare date (`2026-07-01`, read as midnight UTC) and minute-precision stamps (`2026-07-01 09:30`), the forms spreadsheets and mainframe extracts produce.

- `plexuspact check` — validate CSV/Parquet/NDJSON/JSON (file or stdin, gzip/zstd transparent) against a `contract.yaml`; human, JSON, JUnit, and self-contained HTML report outputs; deterministic exit codes (0/1/2/3); `--strict`, `--sample-failures`.
- `plexuspact init` — profile a dataset and draft a contract with inferred types and conservative commented suggestions.
- `plexuspact validate-contract` — lint a contract without data; line/column-pointed errors.
- `plexuspact diff` — semantic contract diff classifying breaking / non-breaking / cosmetic changes; exit 1 on breaking.
- The 20-check core library: `type`, `required`, `columns_exact`/`allow_extra_columns`, `unique`, `min`, `max`, `regex`, `enum`, `length`, `not_empty_string`, `format` (email, uuid, iso_date, iso_datetime, url, country_code_iso2), `row_count_min`/`row_count_max`, `freshness`, `null_ratio_max`, `unique_ratio_min`, `custom_expr`; per-check `severity: error|warn`.
- Reserved v1 schema fields parsed and echoed into results (no engine behavior yet): per-column `pii` and `classification`.
- Dataset-level `consumers`, with an optional `reads` list naming the columns each consumer depends on. Validated against the declared columns — a name that matches no column is an error, because a typo would silently narrow the answer to *who breaks*. Omitting `reads` means the dependency has never been narrowed, which is reported as undeclared rather than as "reads everything". The engine is unchanged: a consumer changes no verdict and `check` ignores the list; `diff` classifies edits to it as cosmetic. It is what blast-radius analysis reads. See [ADR-010](docs/adr/010-reserved-schema-fields.md).
- Parse diagnostics tell a typo apart from a schema this binary predates. Every struct in the model stays `deny_unknown_fields` — a silently ignored `maxx: 5` is the exact mistake this tool exists to catch — so a contract written for a newer PlexusPact fails outright on an older CLI, by design. What changed is the advice: an unrecognized key or value that is *not* a near-miss now names the upgrade path, instead of only offering deletion. Deleting is the one instruction that loses data; a consumer's `reads`, for instance, is its declared dependency, and removing it to satisfy an old binary silently widens that team's blast radius. A plausible typo still gets the plain "did you mean" and no upgrade noise.
- Versioned JSON result schema (`result_schema_version: 1`).
- Official composite GitHub Action (`action/`) with PR annotations and report artifact upload.
- Installers: `install.sh` and `install.ps1`. A Homebrew tap and Scoop bucket land with the first tagged `v0.1.0` release.
- Signed releases: SLSA build provenance attestations for every archive and a keyless (Sigstore/cosign) signature over `SHA256SUMS`.
- Reporting runs to PlexusPact Cloud, built into the CLI. Set `PLEXUSPACT_API_KEY` and `check` records each result against your project on its own — no `curl` pipeline to write or maintain. `plexuspact push <result.json>` does the same for a document already on disk. The verdict is never changed by the reporting: a failed report is a warning on stderr, unless `check --push` explicitly asks for a missing run to be an error. See [ADR-013](docs/adr/013-cloud-reporting-boundary.md).
- `--offline` and `PLEXUSPACT_NO_NETWORK` — implemented, not just documented. Either suppresses all network activity regardless of any API key, so a CI base image can forbid it for everything running underneath.
- `check --no-push` to ignore a configured key for a single run; `--api` / `PLEXUSPACT_API` to point at a self-hosted install.
- GitHub Action inputs `api-key`, `api`, `redact-samples`, `require-push`, and a `run-url` output.
- mdBook documentation site and ADRs 001–010, 013.

[Unreleased]: https://github.com/info-dataplexor/plexuspact/commits/main
