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
- Reserved v1 schema fields parsed and echoed into results (no engine behavior yet): per-column `pii` and `classification`, dataset-level `consumers`.
- Versioned JSON result schema (`result_schema_version: 1`).
- Official composite GitHub Action (`action/`) with PR annotations and report artifact upload.
- Installers: `install.sh` and `install.ps1`. A Homebrew tap and Scoop bucket land with the first tagged `v0.1.0` release.
- Signed releases: SLSA build provenance attestations for every archive and a keyless (Sigstore/cosign) signature over `SHA256SUMS`.
- mdBook documentation site and ADRs 001–010.

[Unreleased]: https://github.com/dataplexor/plexuspact/commits/main
