# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
with 0.x rules: breaking CLI or contract-schema changes bump the minor version
and get a `MIGRATION.md` entry.

## [Unreleased]

### Added

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
