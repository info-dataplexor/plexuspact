# Security Policy

## Reporting a vulnerability

Please email **security@plexuspact.com** — do **not** open a public issue for security reports. Include a description, reproduction steps, and the affected version. We aim to acknowledge within 48 hours and to ship a fix or mitigation for confirmed issues within 30 days. We will credit reporters in the release notes unless you prefer otherwise.

If you cannot use email, GitHub's [private vulnerability reporting](https://github.com/dataplexor/plexuspact/security/advisories/new) on this repository is also monitored.

## Supported versions

Pre-1.0, only the latest minor release line receives security fixes.

| Version | Supported |
|---|---|
| 0.1.x (latest) | Yes |
| older | No — please upgrade |

## Threat model summary

`plexuspact` is designed to be safe to run on untrusted data files in CI:

- **No network I/O in the check path.** `plexuspact check` never opens a socket. Anything network-shaped (opt-in telemetry, if ever shipped; update checks) is separate, clearly logged, disabled by `--offline`, and killable via environment variable (`PLEXUSPACT_NO_TELEMETRY`). The MVP ships with **no telemetry code at all** — see [docs/src/telemetry.md](docs/src/telemetry.md).
- **No code execution from contracts.** `custom_expr` evaluates Polars expressions only — no arbitrary code, no filesystem access from expressions.
- **No panics on user input.** Malformed data or contracts produce diagnostics and exit code 2/3, never a crash; parsers are fuzzed (`cargo-fuzz`) in CI.
- **Sample values may contain PII.** Failure samples in reports quote real data values. Use `--redact-samples` to mask values (row numbers are kept) before sharing reports outside your team.
- **Supply chain.** `cargo deny` (license allowlist + RUSTSEC advisories) runs in CI; releases publish `SHA256SUMS`, and the install scripts and GitHub Action verify checksums before executing anything.
