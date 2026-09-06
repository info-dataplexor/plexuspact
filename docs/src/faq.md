# FAQ

## General

### How is this different from Great Expectations or Soda?

Three ways. **Form factor:** plexuspact is a single ~40 MB static binary with a sub-50 ms cold start — no Python environment, no pip, no JVM — so it runs in the *producer's* CI (an app team's Node or Go repo), which is the whole point of shift-left. **Speed and memory:** the Polars/Arrow engine streams, so a 10 GB CSV validates in constant memory where Pandas-based validators OOM. **Surface area:** one YAML file and 20 checks that cover the common cases, instead of a framework to configure.

### Is it really free? What's the catch?

The CLI — every check, every output format, `init`, `diff`, and quarantine when it ships — is Apache-2.0, free forever. Single-developer productivity is never paywalled. The commercial product (Phase 2) is the hosted registry: contract history, drift charts, alerting, blast-radius lineage, compliance evidence — team coordination features. The CLI never requires an account and works fully offline.

### Does `plexuspact check` send my data anywhere?

Not unless you ask it to, and asking takes a deliberate step: setting `PLEXUSPACT_API_KEY` to a PlexusPact Cloud project key. Then — and only then — each result is [reported to that project](cloud.md), which prints `✓ reported run to …` on stderr every time. With no key set, nothing leaves the machine.

There is no telemetry of any kind, and the validation engine has no network client at all. `--offline` (or `PLEXUSPACT_NO_NETWORK`) guarantees zero sockets regardless of what else is configured. See the [telemetry and network policy](telemetry.md).

### What platforms are supported?

Prebuilt: x86_64 Linux (fully static musl — runs on Alpine, distroless, anything), macOS Intel and Apple Silicon, x86_64 Windows. Anything else with a Rust target: `cargo install --git https://github.com/info-dataplexor/plexuspact plexuspact-cli`.

## Data & formats

### What input formats are supported?

CSV (with delimiter/quote/encoding options), Parquet, NDJSON, and JSON arrays — from a file or stdin (`-`), with transparent gzip and zstd decompression. Format is sniffed from the extension; override with `--input-format`.

### How big a file can it handle?

The engine streams in batches, so memory stays roughly constant regardless of file size — the target is a 10 GB CSV in under 60 s on 2 vCPU / 4 GB RAM. One documented exception: exact `unique` on very high-cardinality columns holds one hash per distinct value (~8 bytes each; 100 M distinct ≈ 800 MB). For those columns, use `unique` with `approx: true` (HyperLogLog, constant memory) — see [ADR-005](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/005-exact-vs-approx-unique.md).

### Can it validate a database table?

Not yet — files and streams only in Phase 1. SQL sources via DataFusion are planned for Phase 2. Meanwhile, many teams pipe an export: `psql -c "COPY (...) TO STDOUT CSV HEADER" | plexuspact check - --input-format csv --contract contract.yaml`.

### A value fails its type — does the run crash?

No. A type mismatch is itself a reported check failure (per `settings.on_type_mismatch`) with sample rows, never a crash. No-panics-on-user-input is a hard rule, enforced by fuzzing.

## Contracts & checks

### Do I have to write contracts by hand?

Start with `plexuspact init data.csv` — it profiles the data and drafts typed columns plus conservative, commented suggestions that the profiled data already passes. Phase 1.5 adds `init --ai` for semantic suggestions (opt-in, bring your own key or local model).

### What are the `pii` and `classification` fields for? They don't seem to do anything.

Correct — they are parsed, validated, and echoed into reports, nothing more. They're reserved in the v1 schema because contract schemas are painful to change once committed across many repos, and compliance-evidence reporting anchors on them. Tag PII now, benefit later. See [ADR-010](https://github.com/info-dataplexor/plexuspact/blob/main/docs/adr/010-reserved-schema-fields.md).

### And `consumers`?

That one does something now. Naming a consumer, and optionally the columns it `reads`, is what lets a tool answer *who breaks* — which teams are downstream of a dropped column or a failing check, and who to contact. `plexuspact check` still ignores it entirely (a consumer changes no verdict, so your exit code is unaffected), but the list is echoed into the result and PlexusPact Cloud reads it on the review screen, on a failing run, and in change alerts. See [consumers](contract-reference.md#consumers).

### A check I need is missing.

First try `custom_expr` — any Polars boolean expression. If that's awkward, [file a feature request](https://github.com/info-dataplexor/plexuspact/issues) with the YAML you wish you could write. Note that some categories are deliberately out of scope for now: anomaly detection / statistical ML checks are Phase 2+, and data *repair* is never in scope (quarantine routes rows; it never mutates them).

### How do I stop someone weakening the contract to make CI pass?

Run `plexuspact diff` in the contract repo's CI: it classifies changes as breaking / semantic / non-breaking / cosmetic and exits 1 on breaking. Combine with CODEOWNERS on the contract files so consumers review changes. Read the semantic ones — a column whose meaning moved fails nothing and quietly falsifies every dashboard built on the old meaning.

## CI & operations

### Warnings are failing my pipeline / not failing my pipeline.

That's the severity system: `warn` failures are reported but exit 0; `--strict` promotes them to exit 1. Details in [exit codes](exit-codes.md).

### Can I fail the job but still keep the good rows?

That's quarantine mode (Phase 1.5, v0.4): `--quarantine bad_rows/` routes failing rows to a dead-letter file with `_row_number` and `_reasons` columns, with an exact reconciliation guarantee (`rows_in == passed + quarantined`), and `--quarantine-exit-zero` for non-blocking operation.

### How do I keep the binary version consistent across CI jobs?

Pin it: the GitHub Action's `version` input, `VERSION=` for `install.sh`, or a `PLEXUSPACT_VERSION` variable in your pipeline. Every release ships `SHA256SUMS`; the installers and the action verify checksums automatically.

## Project

### Why is it called "plexuspact"?

It enforces data *contracts*. (The name is a working title — see the project README.)

### How do I report a security issue?

Email security@plexuspact.com — see [SECURITY.md](https://github.com/info-dataplexor/plexuspact/blob/main/SECURITY.md). Please don't open a public issue.

### How can I contribute?

See [CONTRIBUTING.md](https://github.com/info-dataplexor/plexuspact/blob/main/CONTRIBUTING.md) — dev setup takes two commands, and `good first issue` labels are curated.
